use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use serde_json::Value;
use sqlx::{postgres::PgRow, types::Json as SqlJson, PgPool, Postgres, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use uuid::Uuid;
use vpsman_common::{
    is_terminal_command_type, is_terminal_session_event, payload_hash, terminal_session_state,
    CommandOutput, TerminalStreamOutput, MAX_TERMINAL_FLOW_WINDOW_BYTES,
};

use crate::{
    model::JobOutputView,
    model_terminal::{
        TerminalInputRequestRecord, TerminalOutputChunkRecord, TerminalReplayChunkView,
        TerminalReplayView, TerminalSessionView,
    },
    repository::Repository,
    repository_job_outputs::JobOutputWriteResult,
    ApiError,
};

const TERMINAL_INPUT_ACTIVE_STATUSES: &[&str] = &["reserved", "queued", "dispatching", "running"];

impl Repository {
    pub(crate) async fn reserve_terminal_input_request(
        &self,
        client_id: &str,
        session_id: Uuid,
        job_id: Uuid,
        payload_sha256_hex: &str,
        payload_size_bytes: i64,
    ) -> std::result::Result<TerminalInputRequestRecord, ApiError> {
        match self {
            Self::Memory(memory) => {
                let session = memory
                    .terminal_sessions
                    .read()
                    .await
                    .iter()
                    .find(|session| {
                        session.client_id == client_id && session.session_id == session_id
                    })
                    .cloned()
                    .ok_or_else(|| ApiError::not_found("terminal_session_not_found"))?;
                if session.state != "open" || session.session_exited {
                    return Err(ApiError::conflict("terminal_session_not_open"));
                }
                let mut requests = memory.terminal_input_requests.write().await;
                if let Some(existing) = requests.iter().find(|request| request.job_id == job_id) {
                    if existing.client_id != client_id
                        || existing.session_id != session_id
                        || existing.payload_sha256_hex != payload_sha256_hex
                    {
                        return Err(ApiError::conflict("terminal_input_job_id_conflict"));
                    }
                    return Ok(existing.clone());
                }
                if requests.iter().any(|request| {
                    request.client_id == client_id
                        && request.session_id == session_id
                        && TERMINAL_INPUT_ACTIVE_STATUSES.contains(&request.status.as_str())
                }) {
                    return Err(ApiError::conflict("terminal_input_request_pending"));
                }
                let last_session_seq = session.last_input_seq.unwrap_or(0);
                let last_reserved_seq = requests
                    .iter()
                    .filter(|request| {
                        request.client_id == client_id && request.session_id == session_id
                    })
                    .map(|request| request.input_seq)
                    .max()
                    .unwrap_or(0);
                let now = now_rfc3339();
                let record = TerminalInputRequestRecord {
                    job_id,
                    client_id: client_id.to_string(),
                    session_id,
                    input_seq: last_session_seq.max(last_reserved_seq).saturating_add(1),
                    payload_sha256_hex: payload_sha256_hex.to_string(),
                    status: "reserved".to_string(),
                    updated_at: now,
                    completed_at: None,
                };
                requests.push(record.clone());
                Ok(record)
            }
            Self::Postgres(pool) => {
                let mut tx = pool
                    .begin()
                    .await
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                let session = sqlx::query(
                    r#"
                    SELECT state, session_exited, COALESCE(last_input_seq, 0) AS last_input_seq
                    FROM terminal_sessions
                    WHERE client_id = $1 AND session_id = $2
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .bind(session_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                let Some(session) = session else {
                    return Err(ApiError::not_found("terminal_session_not_found"));
                };
                let state: String = session
                    .try_get("state")
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                let session_exited: bool = session
                    .try_get("session_exited")
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                if state != "open" || session_exited {
                    return Err(ApiError::conflict("terminal_session_not_open"));
                }
                if let Some(existing) =
                    postgres_terminal_input_request_for_job(&mut tx, job_id).await?
                {
                    if existing.client_id != client_id
                        || existing.session_id != session_id
                        || existing.payload_sha256_hex != payload_sha256_hex
                    {
                        return Err(ApiError::conflict("terminal_input_job_id_conflict"));
                    }
                    tx.commit()
                        .await
                        .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                    return Ok(existing);
                }
                let pending: Option<Uuid> = sqlx::query_scalar(
                    r#"
                    SELECT job_id
                    FROM terminal_input_requests
                    WHERE client_id = $1
                      AND session_id = $2
                      AND status IN ('reserved', 'queued', 'dispatching', 'running')
                    ORDER BY input_seq ASC
                    LIMIT 1
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .bind(session_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                if pending.is_some() {
                    return Err(ApiError::conflict("terminal_input_request_pending"));
                }
                let last_session_seq: i64 = session
                    .try_get("last_input_seq")
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                let last_reserved_seq: i64 = sqlx::query_scalar(
                    r#"
                    SELECT COALESCE(MAX(input_seq), 0)
                    FROM terminal_input_requests
                    WHERE client_id = $1 AND session_id = $2
                    "#,
                )
                .bind(client_id)
                .bind(session_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                let input_seq = last_session_seq.max(last_reserved_seq).saturating_add(1);
                let row = sqlx::query(
                    r#"
                    INSERT INTO terminal_input_requests (
                        job_id,
                        client_id,
                        session_id,
                        input_seq,
                        payload_sha256_hex,
                        payload_size_bytes,
                        status
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, 'reserved')
                    RETURNING
                        job_id,
                        client_id,
                        session_id,
                        input_seq,
                        payload_sha256_hex,
                        status,
                        updated_at::text AS updated_at,
                        completed_at::text AS completed_at
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(session_id)
                .bind(input_seq)
                .bind(payload_sha256_hex)
                .bind(payload_size_bytes)
                .fetch_one(&mut *tx)
                .await
                .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                tx.commit()
                    .await
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                terminal_input_request_from_row(row)
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))
            }
        }
    }

    pub(crate) async fn mark_terminal_input_request_status(
        &self,
        job_id: Uuid,
        status: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let now = now_rfc3339();
                if let Some(request) = memory
                    .terminal_input_requests
                    .write()
                    .await
                    .iter_mut()
                    .find(|request| request.job_id == job_id)
                {
                    request.status = status.to_string();
                    request.updated_at = now.clone();
                    if !TERMINAL_INPUT_ACTIVE_STATUSES.contains(&status) {
                        request.completed_at = Some(now);
                    }
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                let completed = !TERMINAL_INPUT_ACTIVE_STATUSES.contains(&status);
                sqlx::query(
                    r#"
                    UPDATE terminal_input_requests
                    SET status = $2,
                        updated_at = now(),
                        completed_at = CASE WHEN $3 THEN COALESCE(completed_at, now()) ELSE completed_at END
                    WHERE job_id = $1
                    "#,
                )
                .bind(job_id)
                .bind(status)
                .bind(completed)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn record_terminal_input_status_output(
        &self,
        job_id: Uuid,
        output: &CommandOutput,
    ) -> Result<()> {
        if output.stream != vpsman_common::OutputStream::Status {
            return Ok(());
        }
        let Ok(value) = serde_json::from_slice::<Value>(&output.data) else {
            return Ok(());
        };
        if value.get("type").and_then(Value::as_str) != Some("terminal_input") {
            return Ok(());
        }
        let Some(status) = value.get("status").and_then(Value::as_str) else {
            return Ok(());
        };
        self.mark_terminal_input_request_status(job_id, status)
            .await
    }

    pub(crate) async fn list_terminal_sessions(
        &self,
        limit: i64,
        client_id: Option<&str>,
        session_id: Option<Uuid>,
    ) -> Result<Vec<TerminalSessionView>> {
        let limit = limit.clamp(1, 200);
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
                let mut sessions = build_terminal_sessions(outputs, limit, session_id);
                sessions.extend(
                    memory
                        .terminal_sessions
                        .read()
                        .await
                        .iter()
                        .filter(|session| {
                            client_id.is_none_or(|client_id| session.client_id == client_id)
                                && session_id
                                    .is_none_or(|session_id| session.session_id == session_id)
                        })
                        .cloned(),
                );
                Ok(deduplicate_terminal_sessions(sessions, limit))
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        session_id,
                        client_id,
                        state,
                        last_status,
                        argv,
                        cwd,
                        cols,
                        rows,
                        idle_timeout_secs,
                        flow_window_bytes,
                        output_first_seq,
                        output_next_seq,
                        output_retained_first_seq,
                        output_retained_bytes,
                        output_dropped_bytes,
                        output_dropped_chunks,
                        output_replay_truncated,
                        last_input_seq,
                        session_exited,
                        close_reason,
                        last_event,
                        last_job_id,
                        last_command_type,
                        last_seq,
                        observed_at::text AS observed_at
                    FROM terminal_sessions
                    WHERE ($2::text IS NULL OR client_id = $2)
                      AND ($3::uuid IS NULL OR session_id = $3)
                    ORDER BY observed_at DESC, client_id ASC, session_id ASC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .bind(client_id)
                .bind(session_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(terminal_session_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }

    pub(crate) async fn terminal_session_replay(
        &self,
        client_id: &str,
        session_id: Uuid,
        from_seq: Option<i64>,
        limit: i64,
        max_bytes: i64,
        include_data: bool,
    ) -> Result<TerminalReplayView> {
        let from_seq = from_seq.unwrap_or(1).max(1);
        let limit = limit.clamp(1, 1000);
        let max_bytes = max_bytes.max(1);
        match self {
            Self::Memory(memory) => {
                let mut chunks = memory
                    .terminal_output_chunks
                    .read()
                    .await
                    .iter()
                    .filter(|chunk| {
                        chunk.client_id == client_id
                            && chunk.session_id == session_id
                            && chunk.terminal_seq >= from_seq
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                chunks.sort_by_key(|chunk| chunk.terminal_seq);
                Ok(build_terminal_replay_from_chunks(
                    client_id,
                    session_id,
                    chunks,
                    from_seq,
                    limit,
                    max_bytes,
                    include_data,
                    memory_terminal_next_seq(memory, client_id, session_id).await,
                ))
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        session_id,
                        terminal_seq,
                        job_id,
                        data,
                        size_bytes,
                        sha256_hex,
                        created_at::text AS created_at
                    FROM terminal_output_chunks
                    WHERE client_id = $1
                      AND session_id = $2
                      AND terminal_seq >= $3
                    ORDER BY terminal_seq ASC
                    LIMIT $4
                    "#,
                )
                .bind(client_id)
                .bind(session_id)
                .bind(from_seq)
                .bind(limit.saturating_add(1))
                .fetch_all(pool)
                .await?;
                let chunks = rows
                    .into_iter()
                    .map(terminal_output_chunk_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
                let next_seq =
                    postgres_terminal_next_seq(pool, client_id, session_id, from_seq).await?;
                Ok(build_terminal_replay_from_chunks(
                    client_id,
                    session_id,
                    chunks,
                    from_seq,
                    limit,
                    max_bytes,
                    include_data,
                    next_seq,
                ))
            }
        }
    }

    pub(crate) async fn record_terminal_stream_chunk(
        &self,
        client_id: &str,
        event: &TerminalStreamOutput,
    ) -> Result<JobOutputWriteResult> {
        let terminal_seq = terminal_seq_i64(
            event
                .terminal_seq
                .context("terminal stream chunk missing terminal_seq")?,
        )?;
        let record = terminal_output_chunk_record(
            client_id,
            event.session_id,
            terminal_seq,
            event.job_id,
            event.output.data.clone(),
            None,
        );
        let retention = TerminalRetentionBounds::from_stream(event)?;
        self.record_terminal_output_chunk_record(record, retention)
            .await
    }

    pub(crate) async fn record_terminal_stream_status(
        &self,
        client_id: &str,
        event: &TerminalStreamOutput,
    ) -> Result<()> {
        let Some(command_type) = self.terminal_job_command_type(event.job_id).await? else {
            anyhow::bail!("terminal_stream_job_not_found");
        };
        let output = TerminalStatusOutput {
            job_id: event.job_id,
            client_id: client_id.to_string(),
            seq: 0,
            data: event.output.data.clone(),
            created_at: now_rfc3339(),
            command_type,
        };
        let Some(event) = parse_terminal_event(output) else {
            anyhow::bail!("invalid_terminal_stream_status");
        };
        self.upsert_terminal_session_event(event).await
    }

    pub(crate) async fn record_terminal_command_replay_chunks(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<()> {
        let outputs = self
            .list_terminal_command_job_outputs(job_id, client_id)
            .await?;
        let Some(status) = terminal_replay_status_for_job_outputs(&outputs) else {
            return Ok(());
        };
        let Some(first_seq) = status.first_seq else {
            return Ok(());
        };
        let Some(session_id) = status.session_id else {
            return Ok(());
        };
        let retention = TerminalRetentionBounds {
            retained_first_seq: status.retained_first_seq.unwrap_or(first_seq).max(1),
            retained_bytes: retention_cap_i64(
                status
                    .retained_bytes
                    .unwrap_or(i64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES)),
            ),
            dropped_bytes: status.dropped_bytes.unwrap_or(0),
            dropped_chunks: status.dropped_chunks.unwrap_or(0),
            replay_truncated: status.replay_truncated,
        };
        let mut pty_index = 0_i64;
        for output in outputs.into_iter().filter(|output| output.stream == "pty") {
            let terminal_seq = first_seq.saturating_add(pty_index);
            pty_index = pty_index.saturating_add(1);
            if terminal_seq < 1
                || status
                    .next_seq
                    .is_some_and(|next_seq| terminal_seq >= next_seq)
            {
                continue;
            }
            let data = BASE64
                .decode(&output.data_base64)
                .context("terminal replay job output is not valid base64")?;
            let record = terminal_output_chunk_record(
                client_id,
                session_id,
                terminal_seq,
                output.job_id,
                data,
                Some(output.created_at),
            );
            let result = self
                .record_terminal_output_chunk_record(record, retention)
                .await?;
            if result == JobOutputWriteResult::DuplicateConflict {
                anyhow::bail!("terminal_output_sequence_conflict");
            }
        }
        Ok(())
    }

    async fn record_terminal_output_chunk_record(
        &self,
        record: TerminalOutputChunkRecord,
        retention: TerminalRetentionBounds,
    ) -> Result<JobOutputWriteResult> {
        let result = match self {
            Self::Memory(memory) => {
                let mut chunks = memory.terminal_output_chunks.write().await;
                let existing = chunks.iter().position(|chunk| {
                    chunk.client_id == record.client_id
                        && chunk.session_id == record.session_id
                        && chunk.terminal_seq == record.terminal_seq
                });
                let result = match existing {
                    Some(index) if terminal_output_chunk_matches(&chunks[index], &record) => {
                        JobOutputWriteResult::DuplicateIdentical
                    }
                    Some(_) => JobOutputWriteResult::DuplicateConflict,
                    None => {
                        chunks.push(record.clone());
                        JobOutputWriteResult::Inserted
                    }
                };
                if result != JobOutputWriteResult::DuplicateConflict {
                    prune_memory_terminal_chunks(
                        &mut chunks,
                        &record.client_id,
                        record.session_id,
                        retention,
                    );
                    drop(chunks);
                    update_memory_terminal_session_range(memory, &record, retention).await;
                }
                result
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let inserted = sqlx::query_scalar::<_, Option<String>>(
                    r#"
                    INSERT INTO terminal_output_chunks (
                        client_id,
                        session_id,
                        terminal_seq,
                        job_id,
                        data,
                        size_bytes,
                        sha256_hex,
                        created_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8::timestamptz)
                    ON CONFLICT (client_id, session_id, terminal_seq)
                    DO NOTHING
                    RETURNING created_at::text
                    "#,
                )
                .bind(&record.client_id)
                .bind(record.session_id)
                .bind(record.terminal_seq)
                .bind(record.job_id)
                .bind(&record.data)
                .bind(record.size_bytes)
                .bind(&record.sha256_hex)
                .bind(&record.created_at)
                .fetch_optional(&mut *tx)
                .await?;
                let result = if inserted.flatten().is_some() {
                    JobOutputWriteResult::Inserted
                } else {
                    let existing = sqlx::query(
                        r#"
                        SELECT data, size_bytes, sha256_hex, created_at::text AS created_at
                        FROM terminal_output_chunks
                        WHERE client_id = $1 AND session_id = $2 AND terminal_seq = $3
                        "#,
                    )
                    .bind(&record.client_id)
                    .bind(record.session_id)
                    .bind(record.terminal_seq)
                    .fetch_one(&mut *tx)
                    .await?;
                    let existing = TerminalOutputChunkRecord {
                        client_id: record.client_id.clone(),
                        session_id: record.session_id,
                        terminal_seq: record.terminal_seq,
                        job_id: record.job_id,
                        data: existing.try_get("data")?,
                        size_bytes: existing.try_get("size_bytes")?,
                        sha256_hex: existing.try_get("sha256_hex")?,
                        created_at: existing.try_get("created_at")?,
                    };
                    if terminal_output_chunk_matches(&existing, &record) {
                        JobOutputWriteResult::DuplicateIdentical
                    } else {
                        JobOutputWriteResult::DuplicateConflict
                    }
                };
                if result == JobOutputWriteResult::DuplicateConflict {
                    tx.rollback().await?;
                    return Ok(JobOutputWriteResult::DuplicateConflict);
                }
                prune_postgres_terminal_chunks(
                    &mut tx,
                    &record.client_id,
                    record.session_id,
                    retention,
                )
                .await?;
                update_postgres_terminal_session_range(
                    &mut tx,
                    &record.client_id,
                    record.session_id,
                    record.terminal_seq.saturating_add(1),
                    retention,
                )
                .await?;
                tx.commit().await?;
                result
            }
        };
        Ok(result)
    }

    async fn terminal_job_command_type(&self, job_id: Uuid) -> Result<Option<String>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .jobs
                .read()
                .await
                .iter()
                .find(|job| job.id == job_id)
                .map(|job| job.command_type.clone())),
            Self::Postgres(pool) => {
                let command_type = sqlx::query_scalar(
                    r#"
                    SELECT command_type
                    FROM jobs
                    WHERE id = $1
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?;
                Ok(command_type)
            }
        }
    }

    async fn list_terminal_command_job_outputs(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<Vec<JobOutputView>> {
        match self {
            Self::Memory(memory) => {
                let mut outputs = memory
                    .job_outputs
                    .read()
                    .await
                    .iter()
                    .filter(|output| output.job_id == job_id && output.client_id == client_id)
                    .cloned()
                    .collect::<Vec<_>>();
                outputs.sort_by_key(|output| output.seq);
                Ok(outputs)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        job_id,
                        client_id,
                        seq,
                        stream,
                        data,
                        storage,
                        object_key,
                        data_sha256_hex,
                        data_size_bytes,
                        exit_code,
                        done,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE job_id = $1 AND client_id = $2
                    ORDER BY seq ASC
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let data: Vec<u8> = row.try_get("data")?;
                        Ok(JobOutputView {
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
                            received_at: None,
                            created_at: row.try_get("created_at")?,
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }

    async fn upsert_terminal_session_event(&self, event: TerminalEvent) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut sessions = memory.terminal_sessions.write().await;
                upsert_memory_terminal_session(&mut sessions, event);
                Ok(())
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO terminal_sessions (
                        session_id,
                        client_id,
                        state,
                        last_status,
                        argv,
                        cwd,
                        cols,
                        rows,
                        idle_timeout_secs,
                        flow_window_bytes,
                        output_first_seq,
                        output_next_seq,
                        output_retained_first_seq,
                        output_retained_bytes,
                        output_dropped_bytes,
                        output_dropped_chunks,
                        output_replay_truncated,
                        last_input_seq,
                        session_exited,
                        close_reason,
                        last_event,
                        last_job_id,
                        last_command_type,
                        last_seq,
                        observed_at
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                        $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
                        $21, $22, $23, $24, $25::timestamptz
                    )
                    ON CONFLICT (client_id, session_id)
                    DO UPDATE SET
                        state = EXCLUDED.state,
                        last_status = EXCLUDED.last_status,
                        argv = CASE
                            WHEN jsonb_array_length(EXCLUDED.argv) > 0 THEN EXCLUDED.argv
                            ELSE terminal_sessions.argv
                        END,
                        cwd = COALESCE(EXCLUDED.cwd, terminal_sessions.cwd),
                        cols = COALESCE(EXCLUDED.cols, terminal_sessions.cols),
                        rows = COALESCE(EXCLUDED.rows, terminal_sessions.rows),
                        idle_timeout_secs = COALESCE(
                            EXCLUDED.idle_timeout_secs,
                            terminal_sessions.idle_timeout_secs
                        ),
                        flow_window_bytes = COALESCE(
                            EXCLUDED.flow_window_bytes,
                            terminal_sessions.flow_window_bytes
                        ),
                        output_first_seq = COALESCE(
                            terminal_sessions.output_first_seq,
                            EXCLUDED.output_first_seq
                        ),
                        output_next_seq = GREATEST(
                            COALESCE(terminal_sessions.output_next_seq, 0),
                            COALESCE(EXCLUDED.output_next_seq, 0)
                        ),
                        output_retained_first_seq = COALESCE(
                            EXCLUDED.output_retained_first_seq,
                            terminal_sessions.output_retained_first_seq
                        ),
                        output_retained_bytes = COALESCE(
                            EXCLUDED.output_retained_bytes,
                            terminal_sessions.output_retained_bytes
                        ),
                        output_dropped_bytes = COALESCE(
                            EXCLUDED.output_dropped_bytes,
                            terminal_sessions.output_dropped_bytes
                        ),
                        output_dropped_chunks = COALESCE(
                            EXCLUDED.output_dropped_chunks,
                            terminal_sessions.output_dropped_chunks
                        ),
                        output_replay_truncated =
                            terminal_sessions.output_replay_truncated
                            OR EXCLUDED.output_replay_truncated,
                        last_input_seq = COALESCE(
                            EXCLUDED.last_input_seq,
                            terminal_sessions.last_input_seq
                        ),
                        session_exited = EXCLUDED.session_exited,
                        close_reason = COALESCE(EXCLUDED.close_reason, terminal_sessions.close_reason),
                        last_event = EXCLUDED.last_event,
                        last_job_id = EXCLUDED.last_job_id,
                        last_command_type = EXCLUDED.last_command_type,
                        last_seq = EXCLUDED.last_seq,
                        observed_at = EXCLUDED.observed_at
                    "#,
                )
                .bind(event.session_id)
                .bind(&event.client_id)
                .bind(event.state)
                .bind(&event.status)
                .bind(SqlJson(&event.argv))
                .bind(&event.cwd)
                .bind(event.cols)
                .bind(event.rows)
                .bind(event.idle_timeout_secs)
                .bind(event.flow_window_bytes)
                .bind(event.output_first_seq)
                .bind(event.output_next_seq)
                .bind(event.output_retained_first_seq)
                .bind(event.output_retained_bytes)
                .bind(event.output_dropped_bytes)
                .bind(event.output_dropped_chunks)
                .bind(event.output_replay_truncated)
                .bind(event.input_seq)
                .bind(event.session_exited)
                .bind(&event.close_reason)
                .bind(&event.event_type)
                .bind(event.job_id)
                .bind(&event.command_type)
                .bind(event.seq)
                .bind(&event.created_at)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn refresh_terminal_sessions_for_client(&self, client_id: &str) -> Result<()> {
        let Self::Postgres(pool) = self else {
            return Ok(());
        };
        let sessions = terminal_sessions_from_outputs(pool, Some(client_id), None, 200).await?;
        for session in sessions {
            sqlx::query(
                r#"
                INSERT INTO terminal_sessions (
                    session_id,
                    client_id,
                    state,
                    last_status,
                    argv,
                    cwd,
                    cols,
                    rows,
                    idle_timeout_secs,
                    flow_window_bytes,
                    output_first_seq,
                    output_next_seq,
                    output_retained_first_seq,
                    output_retained_bytes,
                    output_dropped_bytes,
                    output_dropped_chunks,
                    output_replay_truncated,
                    last_input_seq,
                    session_exited,
                    close_reason,
                    last_event,
                    last_job_id,
                    last_command_type,
                    last_seq,
                    observed_at
                )
                VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                    $11, $12, $13, $14, $15, $16, $17, $18, $19,
                    $20, $21, $22, $23, $24, $25::timestamptz
                )
                ON CONFLICT (client_id, session_id)
                DO UPDATE SET
                    state = EXCLUDED.state,
                    last_status = EXCLUDED.last_status,
                    argv = EXCLUDED.argv,
                    cwd = EXCLUDED.cwd,
                    cols = EXCLUDED.cols,
                    rows = EXCLUDED.rows,
                    idle_timeout_secs = EXCLUDED.idle_timeout_secs,
                    flow_window_bytes = EXCLUDED.flow_window_bytes,
                    output_first_seq = EXCLUDED.output_first_seq,
                    output_next_seq = EXCLUDED.output_next_seq,
                    output_retained_first_seq = EXCLUDED.output_retained_first_seq,
                    output_retained_bytes = EXCLUDED.output_retained_bytes,
                    output_dropped_bytes = EXCLUDED.output_dropped_bytes,
                    output_dropped_chunks = EXCLUDED.output_dropped_chunks,
                    output_replay_truncated = EXCLUDED.output_replay_truncated,
                    last_input_seq = EXCLUDED.last_input_seq,
                    session_exited = EXCLUDED.session_exited,
                    close_reason = EXCLUDED.close_reason,
                    last_event = EXCLUDED.last_event,
                    last_job_id = EXCLUDED.last_job_id,
                    last_command_type = EXCLUDED.last_command_type,
                    last_seq = EXCLUDED.last_seq,
                    observed_at = EXCLUDED.observed_at
                WHERE EXCLUDED.observed_at >= terminal_sessions.observed_at
                "#,
            )
            .bind(session.session_id)
            .bind(&session.client_id)
            .bind(&session.state)
            .bind(&session.last_status)
            .bind(SqlJson(&session.argv))
            .bind(&session.cwd)
            .bind(session.cols)
            .bind(session.rows)
            .bind(session.idle_timeout_secs)
            .bind(session.flow_window_bytes)
            .bind(session.output_first_seq)
            .bind(session.output_next_seq)
            .bind(session.output_retained_first_seq)
            .bind(session.output_retained_bytes)
            .bind(session.output_dropped_bytes)
            .bind(session.output_dropped_chunks)
            .bind(session.output_replay_truncated)
            .bind(session.last_input_seq)
            .bind(session.session_exited)
            .bind(&session.close_reason)
            .bind(&session.last_event)
            .bind(session.last_job_id)
            .bind(&session.last_command_type)
            .bind(session.last_seq)
            .bind(&session.observed_at)
            .execute(pool)
            .await?;
        }
        Ok(())
    }
}

async fn terminal_sessions_from_outputs(
    pool: &PgPool,
    client_id: Option<&str>,
    session_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<TerminalSessionView>> {
    let limit = limit.clamp(1, 200);
    let scan_limit = limit.saturating_mul(64).clamp(100, 10_000);
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

fn terminal_session_from_row(row: PgRow) -> std::result::Result<TerminalSessionView, sqlx::Error> {
    let argv: SqlJson<Vec<String>> = row.try_get("argv")?;
    Ok(TerminalSessionView {
        session_id: row.try_get("session_id")?,
        client_id: row.try_get("client_id")?,
        state: row.try_get("state")?,
        last_status: row.try_get("last_status")?,
        argv: argv.0,
        cwd: row.try_get("cwd")?,
        cols: row.try_get("cols")?,
        rows: row.try_get("rows")?,
        idle_timeout_secs: row.try_get("idle_timeout_secs")?,
        flow_window_bytes: row.try_get("flow_window_bytes")?,
        output_first_seq: row.try_get("output_first_seq")?,
        output_next_seq: row.try_get("output_next_seq")?,
        output_retained_first_seq: row.try_get("output_retained_first_seq")?,
        output_retained_bytes: row.try_get("output_retained_bytes")?,
        output_dropped_bytes: row.try_get("output_dropped_bytes")?,
        output_dropped_chunks: row.try_get("output_dropped_chunks")?,
        output_replay_truncated: row.try_get("output_replay_truncated")?,
        last_input_seq: row.try_get("last_input_seq")?,
        session_exited: row.try_get("session_exited")?,
        close_reason: row.try_get("close_reason")?,
        last_event: row.try_get("last_event")?,
        last_job_id: row.try_get("last_job_id")?,
        last_command_type: row.try_get("last_command_type")?,
        last_seq: row.try_get("last_seq")?,
        observed_at: row.try_get("observed_at")?,
    })
}

fn terminal_output_chunk_from_row(
    row: PgRow,
) -> std::result::Result<TerminalOutputChunkRecord, sqlx::Error> {
    Ok(TerminalOutputChunkRecord {
        client_id: row.try_get("client_id")?,
        session_id: row.try_get("session_id")?,
        terminal_seq: row.try_get("terminal_seq")?,
        job_id: row.try_get("job_id")?,
        data: row.try_get("data")?,
        size_bytes: row.try_get("size_bytes")?,
        sha256_hex: row.try_get("sha256_hex")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn postgres_terminal_input_request_for_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> std::result::Result<Option<TerminalInputRequestRecord>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT
            job_id,
            client_id,
            session_id,
            input_seq,
            payload_sha256_hex,
            status,
            updated_at::text AS updated_at,
            completed_at::text AS completed_at
        FROM terminal_input_requests
        WHERE job_id = $1
        FOR UPDATE
        "#,
    )
    .bind(job_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
    row.map(terminal_input_request_from_row)
        .transpose()
        .map_err(|error| ApiError::from(anyhow::Error::from(error)))
}

fn terminal_input_request_from_row(
    row: PgRow,
) -> std::result::Result<TerminalInputRequestRecord, sqlx::Error> {
    Ok(TerminalInputRequestRecord {
        job_id: row.try_get("job_id")?,
        client_id: row.try_get("client_id")?,
        session_id: row.try_get("session_id")?,
        input_seq: row.try_get("input_seq")?,
        payload_sha256_hex: row.try_get("payload_sha256_hex")?,
        status: row.try_get("status")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
    })
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
    terminal_session_state(event_type, status, session_exited)
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
    is_terminal_command_type(command_type)
}

fn is_terminal_status_event(event_type: &str) -> bool {
    is_terminal_session_event(event_type)
}

#[derive(Clone, Debug)]
struct TerminalReplayStatus {
    session_id: Option<Uuid>,
    first_seq: Option<i64>,
    next_seq: Option<i64>,
    retained_first_seq: Option<i64>,
    retained_bytes: Option<i64>,
    dropped_bytes: Option<i64>,
    dropped_chunks: Option<i64>,
    replay_truncated: bool,
}

#[derive(Clone, Copy, Debug)]
struct TerminalRetentionBounds {
    retained_first_seq: i64,
    retained_bytes: i64,
    dropped_bytes: i64,
    dropped_chunks: i64,
    replay_truncated: bool,
}

impl TerminalRetentionBounds {
    fn from_stream(event: &TerminalStreamOutput) -> Result<Self> {
        Ok(Self {
            retained_first_seq: event
                .output_retained_first_seq
                .map(terminal_seq_i64)
                .transpose()?
                .unwrap_or(1)
                .max(1),
            retained_bytes: retention_cap_bytes(event.output_retained_bytes),
            dropped_bytes: u64_to_i64_saturating(event.output_dropped_bytes),
            dropped_chunks: u64_to_i64_saturating(event.output_dropped_chunks),
            replay_truncated: event.output_replay_truncated,
        })
    }
}

fn build_terminal_replay_from_chunks(
    client_id: &str,
    session_id: Uuid,
    mut chunks: Vec<TerminalOutputChunkRecord>,
    from_seq: i64,
    limit: i64,
    max_bytes: i64,
    include_data: bool,
    next_seq_hint: i64,
) -> TerminalReplayView {
    let limit = limit.clamp(1, 1000) as usize;
    let mut byte_count = 0_i64;
    let mut replay_chunks = Vec::new();
    let mut truncated = false;
    chunks.sort_by_key(|chunk| chunk.terminal_seq);
    for chunk in chunks {
        if chunk.terminal_seq < from_seq {
            continue;
        }
        if replay_chunks.len() >= limit {
            truncated = true;
            break;
        }
        let size_bytes = chunk.size_bytes.max(0);
        if byte_count.saturating_add(size_bytes) > max_bytes {
            truncated = true;
            break;
        }
        byte_count = byte_count.saturating_add(size_bytes);
        replay_chunks.push(TerminalReplayChunkView {
            terminal_seq: chunk.terminal_seq,
            job_id: chunk.job_id,
            data_base64: include_data.then(|| BASE64.encode(&chunk.data)),
            size_bytes,
            sha256_hex: chunk.sha256_hex,
            created_at: chunk.created_at,
        });
    }
    let available_first_seq = replay_chunks.first().map(|chunk| chunk.terminal_seq);
    let next_seq = next_seq_hint.max(
        replay_chunks
            .last()
            .map(|chunk| chunk.terminal_seq.saturating_add(1))
            .unwrap_or(from_seq),
    );
    TerminalReplayView {
        session_id,
        client_id: client_id.to_string(),
        from_seq,
        available_first_seq,
        next_seq,
        chunk_count: replay_chunks.len(),
        byte_count,
        truncated,
        source: "terminal_output_chunks".to_string(),
        chunks: replay_chunks,
    }
}

fn terminal_replay_status_for_job_outputs(
    outputs: &[JobOutputView],
) -> Option<TerminalReplayStatus> {
    let mut merged = TerminalReplayStatus {
        session_id: None,
        first_seq: None,
        next_seq: None,
        retained_first_seq: None,
        retained_bytes: None,
        dropped_bytes: None,
        dropped_chunks: None,
        replay_truncated: false,
    };
    let mut found = false;
    for status in outputs.iter().filter_map(|output| {
        if output.stream != "status" {
            return None;
        }
        parse_terminal_replay_status(output)
    }) {
        found = true;
        merged.session_id = merged.session_id.or(status.session_id);
        merged.first_seq = match (merged.first_seq, status.first_seq) {
            (Some(current), Some(next)) => Some(current.min(next)),
            (None, value) | (value, None) => value,
        };
        merged.next_seq = match (merged.next_seq, status.next_seq) {
            (Some(current), Some(next)) => Some(current.max(next)),
            (None, value) | (value, None) => value,
        };
        merged.retained_first_seq = status.retained_first_seq.or(merged.retained_first_seq);
        merged.retained_bytes = status.retained_bytes.or(merged.retained_bytes);
        merged.dropped_bytes = status.dropped_bytes.or(merged.dropped_bytes);
        merged.dropped_chunks = status.dropped_chunks.or(merged.dropped_chunks);
        merged.replay_truncated |= status.replay_truncated;
    }
    found.then_some(merged)
}

fn parse_terminal_replay_status(output: &JobOutputView) -> Option<TerminalReplayStatus> {
    let data = BASE64.decode(&output.data_base64).ok()?;
    let value = serde_json::from_slice::<Value>(&data).ok()?;
    if !is_terminal_status_event(value.get("type")?.as_str()?) {
        return None;
    }
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    Some(TerminalReplayStatus {
        session_id: Some(session_id),
        first_seq: value.get("output_first_seq").and_then(json_i64),
        next_seq: value.get("output_next_seq").and_then(json_i64),
        retained_first_seq: value.get("output_retained_first_seq").and_then(json_i64),
        retained_bytes: value.get("output_retained_bytes").and_then(json_i64),
        dropped_bytes: value.get("output_dropped_bytes").and_then(json_i64),
        dropped_chunks: value.get("output_dropped_chunks").and_then(json_i64),
        replay_truncated: value
            .get("output_replay_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

async fn memory_terminal_next_seq(
    memory: &crate::repository::MemoryState,
    client_id: &str,
    session_id: Uuid,
) -> i64 {
    let session_next = memory
        .terminal_sessions
        .read()
        .await
        .iter()
        .find(|session| session.client_id == client_id && session.session_id == session_id)
        .and_then(|session| session.output_next_seq);
    let chunk_next = memory
        .terminal_output_chunks
        .read()
        .await
        .iter()
        .filter(|chunk| chunk.client_id == client_id && chunk.session_id == session_id)
        .map(|chunk| chunk.terminal_seq.saturating_add(1))
        .max();
    session_next.or(chunk_next).unwrap_or(1).max(1)
}

async fn postgres_terminal_next_seq(
    pool: &PgPool,
    client_id: &str,
    session_id: Uuid,
    from_seq: i64,
) -> Result<i64> {
    let next_seq: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(
            (
                SELECT output_next_seq
                FROM terminal_sessions
                WHERE client_id = $1 AND session_id = $2
            ),
            (
                SELECT MAX(terminal_seq) + 1
                FROM terminal_output_chunks
                WHERE client_id = $1 AND session_id = $2
            )
        )
        "#,
    )
    .bind(client_id)
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    Ok(next_seq.unwrap_or(from_seq).max(1))
}

fn terminal_output_chunk_record(
    client_id: &str,
    session_id: Uuid,
    terminal_seq: i64,
    job_id: Uuid,
    data: Vec<u8>,
    created_at: Option<String>,
) -> TerminalOutputChunkRecord {
    TerminalOutputChunkRecord {
        client_id: client_id.to_string(),
        session_id,
        terminal_seq,
        job_id,
        size_bytes: data.len() as i64,
        sha256_hex: payload_hash(&data),
        data,
        created_at: created_at.unwrap_or_else(now_rfc3339),
    }
}

fn terminal_output_chunk_matches(
    left: &TerminalOutputChunkRecord,
    right: &TerminalOutputChunkRecord,
) -> bool {
    left.size_bytes == right.size_bytes
        && left.sha256_hex == right.sha256_hex
        && left.data == right.data
}

fn prune_memory_terminal_chunks(
    chunks: &mut Vec<TerminalOutputChunkRecord>,
    client_id: &str,
    session_id: Uuid,
    retention: TerminalRetentionBounds,
) {
    let mut retained_bytes = 0_i64;
    let mut retained = HashSet::new();
    let mut matching = chunks
        .iter()
        .filter(|chunk| chunk.client_id == client_id && chunk.session_id == session_id)
        .map(|chunk| chunk.terminal_seq)
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| right.cmp(left));
    for terminal_seq in matching {
        if terminal_seq < retention.retained_first_seq {
            continue;
        }
        let Some(size_bytes) = chunks
            .iter()
            .find(|chunk| {
                chunk.client_id == client_id
                    && chunk.session_id == session_id
                    && chunk.terminal_seq == terminal_seq
            })
            .map(|chunk| chunk.size_bytes.max(0))
        else {
            continue;
        };
        if retained_bytes.saturating_add(size_bytes) > retention.retained_bytes {
            continue;
        }
        retained_bytes = retained_bytes.saturating_add(size_bytes);
        retained.insert(terminal_seq);
    }
    chunks.retain(|chunk| {
        chunk.client_id != client_id
            || chunk.session_id != session_id
            || retained.contains(&chunk.terminal_seq)
    });
}

async fn prune_postgres_terminal_chunks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    session_id: Uuid,
    retention: TerminalRetentionBounds,
) -> Result<()> {
    sqlx::query(
        r#"
        WITH ranked AS (
            SELECT
                terminal_seq,
                SUM(size_bytes) OVER (ORDER BY terminal_seq DESC) AS newest_bytes
            FROM terminal_output_chunks
            WHERE client_id = $1 AND session_id = $2
        )
        DELETE FROM terminal_output_chunks chunk
        USING ranked
        WHERE chunk.client_id = $1
          AND chunk.session_id = $2
          AND chunk.terminal_seq = ranked.terminal_seq
          AND (
              chunk.terminal_seq < $3
              OR ranked.newest_bytes > $4
          )
        "#,
    )
    .bind(client_id)
    .bind(session_id)
    .bind(retention.retained_first_seq)
    .bind(retention.retained_bytes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn update_memory_terminal_session_range(
    memory: &crate::repository::MemoryState,
    record: &TerminalOutputChunkRecord,
    retention: TerminalRetentionBounds,
) {
    let mut sessions = memory.terminal_sessions.write().await;
    if let Some(session) = sessions.iter_mut().find(|session| {
        session.client_id == record.client_id && session.session_id == record.session_id
    }) {
        session.output_first_seq = session.output_first_seq.or(Some(1));
        session.output_next_seq = Some(
            session
                .output_next_seq
                .unwrap_or(1)
                .max(record.terminal_seq.saturating_add(1)),
        );
        session.output_retained_first_seq = Some(retention.retained_first_seq);
        session.output_retained_bytes = Some(retention.retained_bytes);
        session.output_dropped_bytes = Some(retention.dropped_bytes);
        session.output_dropped_chunks = Some(retention.dropped_chunks);
        session.output_replay_truncated |= retention.replay_truncated;
    }
}

async fn update_postgres_terminal_session_range(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    session_id: Uuid,
    next_seq: i64,
    retention: TerminalRetentionBounds,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE terminal_sessions
        SET
            output_first_seq = COALESCE(output_first_seq, 1),
            output_next_seq = GREATEST(COALESCE(output_next_seq, 1), $3),
            output_retained_first_seq = $4,
            output_retained_bytes = $5,
            output_dropped_bytes = $6,
            output_dropped_chunks = $7,
            output_replay_truncated = output_replay_truncated OR $8
        WHERE client_id = $1 AND session_id = $2
        "#,
    )
    .bind(client_id)
    .bind(session_id)
    .bind(next_seq.max(1))
    .bind(retention.retained_first_seq)
    .bind(retention.retained_bytes)
    .bind(retention.dropped_bytes)
    .bind(retention.dropped_chunks)
    .bind(retention.replay_truncated)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn upsert_memory_terminal_session(sessions: &mut Vec<TerminalSessionView>, event: TerminalEvent) {
    let next = TerminalAggregate::new(event).into_view();
    if let Some(existing) = sessions.iter_mut().find(|session| {
        session.client_id == next.client_id && session.session_id == next.session_id
    }) {
        existing.state = next.state;
        existing.last_status = next.last_status;
        if !next.argv.is_empty() {
            existing.argv = next.argv;
        }
        existing.cwd = next.cwd.or_else(|| existing.cwd.take());
        existing.cols = next.cols.or(existing.cols);
        existing.rows = next.rows.or(existing.rows);
        existing.idle_timeout_secs = next.idle_timeout_secs.or(existing.idle_timeout_secs);
        existing.flow_window_bytes = next.flow_window_bytes.or(existing.flow_window_bytes);
        existing.output_first_seq = existing.output_first_seq.or(next.output_first_seq);
        existing.output_next_seq = match (existing.output_next_seq, next.output_next_seq) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, value) | (value, None) => value,
        };
        existing.output_retained_first_seq = next
            .output_retained_first_seq
            .or(existing.output_retained_first_seq);
        existing.output_retained_bytes = next
            .output_retained_bytes
            .or(existing.output_retained_bytes);
        existing.output_dropped_bytes = next.output_dropped_bytes.or(existing.output_dropped_bytes);
        existing.output_dropped_chunks = next
            .output_dropped_chunks
            .or(existing.output_dropped_chunks);
        existing.output_replay_truncated |= next.output_replay_truncated;
        existing.last_input_seq = next.last_input_seq.or(existing.last_input_seq);
        existing.session_exited = next.session_exited;
        existing.close_reason = next.close_reason.or_else(|| existing.close_reason.take());
        existing.last_event = next.last_event;
        existing.last_job_id = next.last_job_id;
        existing.last_command_type = next.last_command_type;
        existing.last_seq = next.last_seq;
        existing.observed_at = next.observed_at;
    } else {
        sessions.push(next);
    }
}

fn deduplicate_terminal_sessions(
    mut sessions: Vec<TerminalSessionView>,
    limit: i64,
) -> Vec<TerminalSessionView> {
    sessions.sort_by(|left, right| {
        right
            .observed_at
            .cmp(&left.observed_at)
            .then_with(|| left.client_id.cmp(&right.client_id))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    let mut emitted = HashSet::new();
    let mut deduped = Vec::new();
    for session in sessions {
        if emitted.insert((session.client_id.clone(), session.session_id)) {
            deduped.push(session);
            if deduped.len() >= limit.clamp(1, 200) as usize {
                break;
            }
        }
    }
    deduped
}

fn terminal_seq_i64(value: u64) -> Result<i64> {
    i64::try_from(value).context("terminal sequence exceeds i64 range")
}

fn u64_to_i64_saturating(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn retention_cap_bytes(value: u64) -> i64 {
    let value = value.min(u64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES));
    if value == 0 {
        i64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES)
    } else {
        u64_to_i64_saturating(value)
    }
}

fn retention_cap_i64(value: i64) -> i64 {
    if value <= 0 {
        i64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES)
    } else {
        value.min(i64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES))
    }
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::{build_terminal_replay_from_chunks, build_terminal_sessions, TerminalStatusOutput};
    use crate::{
        model_terminal::{TerminalOutputChunkRecord, TerminalSessionView},
        repository::{MemoryState, Repository},
    };
    use uuid::Uuid;
    use vpsman_common::{CommandOutput, OutputStream};

    #[tokio::test]
    async fn memory_terminal_input_reservation_serializes_per_session() {
        let repo = Repository::Memory(MemoryState::default());
        let session_id = Uuid::new_v4();
        insert_terminal_session(
            &repo,
            test_terminal_session("edge-a", session_id, Some(4), "open", false),
        )
        .await;

        let payload_hash = vpsman_common::payload_hash(b"uptime\n");
        let job_id = Uuid::new_v4();
        let first = repo
            .reserve_terminal_input_request("edge-a", session_id, job_id, &payload_hash, 7)
            .await
            .unwrap();
        assert_eq!(first.input_seq, 5);
        assert_eq!(first.status, "reserved");

        let duplicate = repo
            .reserve_terminal_input_request("edge-a", session_id, job_id, &payload_hash, 7)
            .await
            .unwrap();
        assert_eq!(duplicate.input_seq, first.input_seq);

        let conflict = repo
            .reserve_terminal_input_request(
                "edge-a",
                session_id,
                job_id,
                &vpsman_common::payload_hash(b"whoami\n"),
                7,
            )
            .await
            .unwrap_err();
        assert_eq!(conflict.code, "terminal_input_job_id_conflict");

        let pending = repo
            .reserve_terminal_input_request(
                "edge-a",
                session_id,
                Uuid::new_v4(),
                &vpsman_common::payload_hash(b"date\n"),
                5,
            )
            .await
            .unwrap_err();
        assert_eq!(pending.code, "terminal_input_request_pending");

        repo.mark_terminal_input_request_status(job_id, "accepted")
            .await
            .unwrap();
        let next = repo
            .reserve_terminal_input_request(
                "edge-a",
                session_id,
                Uuid::new_v4(),
                &vpsman_common::payload_hash(b"date\n"),
                5,
            )
            .await
            .unwrap();
        assert_eq!(next.input_seq, 6);

        let Repository::Memory(memory) = &repo else {
            unreachable!("memory repository expected");
        };
        let requests = memory.terminal_input_requests.read().await;
        let completed = requests
            .iter()
            .find(|request| request.job_id == job_id)
            .unwrap();
        assert_eq!(completed.status, "accepted");
        assert!(completed.completed_at.is_some());
    }

    #[tokio::test]
    async fn memory_terminal_input_reservation_rejects_missing_or_closed_sessions() {
        let repo = Repository::Memory(MemoryState::default());
        let session_id = Uuid::new_v4();
        let payload_hash = vpsman_common::payload_hash(b"uptime\n");

        let missing = repo
            .reserve_terminal_input_request("edge-a", session_id, Uuid::new_v4(), &payload_hash, 7)
            .await
            .unwrap_err();
        assert_eq!(missing.code, "terminal_session_not_found");

        insert_terminal_session(
            &repo,
            test_terminal_session("edge-a", session_id, None, "closed", true),
        )
        .await;
        let closed = repo
            .reserve_terminal_input_request("edge-a", session_id, Uuid::new_v4(), &payload_hash, 7)
            .await
            .unwrap_err();
        assert_eq!(closed.code, "terminal_session_not_open");
    }

    #[tokio::test]
    async fn memory_terminal_input_status_output_updates_request_state() {
        let repo = Repository::Memory(MemoryState::default());
        let session_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        insert_terminal_session(
            &repo,
            test_terminal_session("edge-a", session_id, None, "open", false),
        )
        .await;
        repo.reserve_terminal_input_request(
            "edge-a",
            session_id,
            job_id,
            &vpsman_common::payload_hash(b"uptime\n"),
            7,
        )
        .await
        .unwrap();

        repo.record_terminal_input_status_output(
            job_id,
            &CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "terminal_input",
                    "status": "duplicate_conflict",
                    "session_id": session_id,
                    "input_seq": 1
                }))
                .unwrap(),
                exit_code: Some(31),
                done: true,
            },
        )
        .await
        .unwrap();

        let Repository::Memory(memory) = &repo else {
            unreachable!("memory repository expected");
        };
        let requests = memory.terminal_input_requests.read().await;
        let request = requests
            .iter()
            .find(|request| request.job_id == job_id)
            .unwrap();
        assert_eq!(request.status, "duplicate_conflict");
        assert!(request.completed_at.is_some());
    }

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
            replay_chunk(input_job, "edge-a", session_id, 1, "100", b"one\n"),
            replay_chunk(input_job, "edge-a", session_id, 2, "100", b"two\n"),
            replay_chunk(poll_job, "edge-a", session_id, 3, "200", b"three\n"),
        ];

        let replay =
            build_terminal_replay_from_chunks("edge-a", session_id, outputs, 2, 10, 1000, true, 4);

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
            replay_chunk(job_id, "edge-a", session_id, 1, "100", b"one"),
            replay_chunk(job_id, "edge-a", session_id, 2, "100", b"two"),
        ];

        let replay =
            build_terminal_replay_from_chunks("edge-a", session_id, outputs, 1, 1, 1000, true, 3);

        assert_eq!(replay.chunk_count, 1);
        assert_eq!(replay.byte_count, 3);
        assert!(replay.truncated);
    }

    #[test]
    fn terminal_replay_metadata_only_omits_data_and_applies_byte_cap() {
        let session_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        let outputs = vec![
            replay_chunk(job_id, "edge-a", session_id, 1, "100", b"one"),
            replay_chunk(job_id, "edge-a", session_id, 2, "101", b"two"),
        ];

        let replay =
            build_terminal_replay_from_chunks("edge-a", session_id, outputs, 1, 10, 3, false, 3);

        assert_eq!(replay.chunk_count, 1);
        assert_eq!(replay.byte_count, 3);
        assert!(replay.truncated);
        assert_eq!(replay.chunks[0].terminal_seq, 1);
        assert!(replay.chunks[0].data_base64.is_none());
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

    fn replay_chunk(
        job_id: Uuid,
        client_id: &str,
        session_id: Uuid,
        terminal_seq: i64,
        created_at: &str,
        data: &[u8],
    ) -> TerminalOutputChunkRecord {
        TerminalOutputChunkRecord {
            client_id: client_id.to_string(),
            session_id,
            terminal_seq,
            job_id,
            data: data.to_vec(),
            size_bytes: data.len() as i64,
            sha256_hex: vpsman_common::payload_hash(data),
            created_at: created_at.to_string(),
        }
    }

    async fn insert_terminal_session(repo: &Repository, session: TerminalSessionView) {
        let Repository::Memory(memory) = repo else {
            unreachable!("memory repository expected");
        };
        memory.terminal_sessions.write().await.push(session);
    }

    fn test_terminal_session(
        client_id: &str,
        session_id: Uuid,
        last_input_seq: Option<i64>,
        state: &str,
        session_exited: bool,
    ) -> TerminalSessionView {
        TerminalSessionView {
            session_id,
            client_id: client_id.to_string(),
            state: state.to_string(),
            last_status: if state == "open" {
                "accepted"
            } else {
                "closed"
            }
            .to_string(),
            argv: vec!["/bin/sh".to_string(), "-l".to_string()],
            cwd: Some("/root".to_string()),
            cols: Some(120),
            rows: Some(40),
            idle_timeout_secs: Some(3600),
            flow_window_bytes: Some(65_536),
            output_first_seq: Some(1),
            output_next_seq: Some(1),
            output_retained_first_seq: Some(1),
            output_retained_bytes: Some(0),
            output_dropped_bytes: Some(0),
            output_dropped_chunks: Some(0),
            output_replay_truncated: false,
            last_input_seq,
            session_exited,
            close_reason: None,
            last_event: state.to_string(),
            last_job_id: Uuid::new_v4(),
            last_command_type: "terminal_open".to_string(),
            last_seq: 0,
            observed_at: "2026-06-21T00:00:00Z".to_string(),
        }
    }
}
