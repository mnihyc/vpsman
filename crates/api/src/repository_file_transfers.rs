use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Row};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
use vpsman_common::{
    file_transfer_session_status, is_file_transfer_command_type, is_file_transfer_session_event,
};

use crate::{
    model::JobOutputView, model_file_transfer::FileTransferSessionView, repository::Repository,
};

const HANDOFF_EVIDENCE_ARTIFACT_AVAILABLE: &str = "artifact_available";
const HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE: &str = "retained_outputs_available";
const HANDOFF_EVIDENCE_NOT_APPLICABLE: &str = "not_applicable";
const HANDOFF_EVIDENCE_NOT_COMPLETED: &str = "not_completed";
const HANDOFF_EVIDENCE_MISSING_FINAL_METADATA: &str = "missing_final_metadata";
const HANDOFF_EVIDENCE_RETAINED_OUTPUTS_PRUNED: &str = "retained_outputs_pruned";
const HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE: &str = "retained_outputs_incomplete";
const HANDOFF_EVIDENCE_RETAINED_OUTPUTS_CONFLICT: &str = "retained_outputs_conflict";

impl Repository {
    pub(crate) async fn list_file_transfer_sessions(
        &self,
        limit: i64,
        client_id: Option<&str>,
        session_id: Option<Uuid>,
    ) -> Result<Vec<FileTransferSessionView>> {
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
                        if !is_file_transfer_command(command_type) {
                            return None;
                        }
                        Some(FileTransferStatusOutput {
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
                Ok(build_file_transfer_sessions(outputs, limit, session_id))
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        session_id,
                        client_id,
                        direction,
                        status,
                        path,
                        size_bytes,
                        progress_bytes,
                        progress_ratio,
                        sha256_hex,
                        chunk_size_bytes,
                        last_chunk_size_bytes,
                        last_chunk_sha256_hex,
                        rate_limit_kbps,
                        resumed,
                        last_event,
                        last_job_id,
                        last_command_type,
                        last_seq,
                        observed_at::text AS observed_at,
                        handoff_available,
                        handoff_object_key,
                        handoff_download_path
                    FROM file_transfer_sessions
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
                    .map(file_transfer_session_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }

    pub(crate) async fn annotate_file_transfer_handoff_evidence(
        &self,
        sessions: &mut [FileTransferSessionView],
    ) -> Result<()> {
        for session in sessions {
            let Some((sha256_hex, size_bytes)) = reset_and_validate_handoff_session(session) else {
                continue;
            };
            let object_key = file_transfer_handoff_object_key(
                &session.client_id,
                session.session_id,
                &sha256_hex,
            );
            let download_path =
                file_transfer_handoff_download_path(&session.client_id, session.session_id);
            if self
                .active_server_artifact_matches(
                    "file_transfer_handoff",
                    &object_key,
                    &sha256_hex,
                    size_bytes,
                )
                .await?
            {
                set_handoff_evidence(
                    session,
                    true,
                    HANDOFF_EVIDENCE_ARTIFACT_AVAILABLE,
                    None,
                    Some(object_key),
                    Some(download_path),
                );
                continue;
            }
            let chunks = self
                .list_file_transfer_download_handoff_chunks(&session.client_id, session.session_id)
                .await?;
            let evidence = self
                .assess_handoff_chunk_evidence(&chunks, size_bytes)
                .await?;
            if evidence.available {
                set_handoff_evidence(
                    session,
                    true,
                    HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE,
                    None,
                    Some(object_key),
                    Some(download_path),
                );
            } else {
                set_handoff_evidence(session, false, evidence.status, evidence.reason, None, None);
            }
        }
        Ok(())
    }

    async fn assess_handoff_chunk_evidence(
        &self,
        chunks: &[FileTransferDownloadHandoffChunk],
        expected_size_bytes: i64,
    ) -> Result<HandoffChunkEvidence> {
        if expected_size_bytes == 0 && chunks.is_empty() {
            return Ok(HandoffChunkEvidence::available());
        }
        if chunks.is_empty() {
            return Ok(HandoffChunkEvidence::unavailable(
                HANDOFF_EVIDENCE_RETAINED_OUTPUTS_PRUNED,
                "retained_chunk_outputs_pruned",
            ));
        }
        let mut by_offset = BTreeMap::<i64, HandoffOffsetEvidence>::new();
        for chunk in chunks {
            if chunk.offset < 0 || chunk.size_bytes <= 0 {
                return Ok(HandoffChunkEvidence::unavailable(
                    HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE,
                    "chunk_metadata_invalid",
                ));
            }
            let output_available = self.handoff_chunk_outputs_available(chunk).await?;
            match by_offset.get_mut(&chunk.offset) {
                Some(existing) => {
                    if existing.size_bytes != chunk.size_bytes
                        || existing.sha256_hex != chunk.sha256_hex
                    {
                        return Ok(HandoffChunkEvidence::unavailable(
                            HANDOFF_EVIDENCE_RETAINED_OUTPUTS_CONFLICT,
                            "duplicate_offset_conflict",
                        ));
                    }
                    existing.output_available |= output_available;
                }
                None => {
                    by_offset.insert(
                        chunk.offset,
                        HandoffOffsetEvidence {
                            size_bytes: chunk.size_bytes,
                            sha256_hex: chunk.sha256_hex.clone(),
                            output_available,
                        },
                    );
                }
            }
        }
        let mut next_offset = 0_i64;
        for (offset, evidence) in by_offset {
            if offset != next_offset {
                return Ok(HandoffChunkEvidence::unavailable(
                    HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE,
                    "chunk_gap",
                ));
            }
            if !evidence.output_available {
                return Ok(HandoffChunkEvidence::unavailable(
                    HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE,
                    "chunk_output_unavailable",
                ));
            }
            next_offset = next_offset.saturating_add(evidence.size_bytes);
        }
        if next_offset != expected_size_bytes {
            return Ok(HandoffChunkEvidence::unavailable(
                HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE,
                "final_size_mismatch",
            ));
        }
        Ok(HandoffChunkEvidence::available())
    }

    async fn handoff_chunk_outputs_available(
        &self,
        chunk: &FileTransferDownloadHandoffChunk,
    ) -> Result<bool> {
        if chunk.outputs.is_empty() {
            return Ok(false);
        }
        let mut size_bytes = 0_i64;
        for output in &chunk.outputs {
            match output.storage.as_str() {
                "inline" => {
                    let Ok(data) = BASE64.decode(&output.data_base64) else {
                        return Ok(false);
                    };
                    size_bytes = size_bytes.saturating_add(data.len() as i64);
                }
                "object_store" => {
                    let Some(object_key) = output.artifact_object_key.as_deref() else {
                        return Ok(false);
                    };
                    let Some(sha256_hex) = output.artifact_sha256_hex.as_deref() else {
                        return Ok(false);
                    };
                    let Some(part_size) = output.artifact_size_bytes else {
                        return Ok(false);
                    };
                    if !self
                        .active_server_artifact_matches(
                            "job_output",
                            object_key,
                            sha256_hex,
                            part_size,
                        )
                        .await?
                    {
                        return Ok(false);
                    }
                    size_bytes = size_bytes.saturating_add(part_size);
                }
                _ => return Ok(false),
            }
            if size_bytes > chunk.size_bytes {
                return Ok(false);
            }
        }
        Ok(size_bytes == chunk.size_bytes)
    }

    pub(crate) async fn refresh_file_transfer_sessions_for_client(
        &self,
        client_id: &str,
    ) -> Result<()> {
        let Self::Postgres(pool) = self else {
            return Ok(());
        };
        let sessions =
            file_transfer_sessions_from_outputs(pool, Some(client_id), None, 200).await?;
        for session in sessions {
            sqlx::query(
                r#"
                INSERT INTO file_transfer_sessions (
                    session_id,
                    client_id,
                    direction,
                    status,
                    path,
                    size_bytes,
                    progress_bytes,
                    progress_ratio,
                    sha256_hex,
                    chunk_size_bytes,
                    last_chunk_size_bytes,
                    last_chunk_sha256_hex,
                    rate_limit_kbps,
                    resumed,
                    last_event,
                    last_job_id,
                    last_command_type,
                    last_seq,
                    observed_at,
                    handoff_available,
                    handoff_object_key,
                    handoff_download_path
                )
                VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                    $12, $13, $14, $15, $16, $17, $18, $19::timestamptz,
                    $20, $21, $22
                )
                ON CONFLICT (client_id, session_id)
                DO UPDATE SET
                    direction = EXCLUDED.direction,
                    status = EXCLUDED.status,
                    path = EXCLUDED.path,
                    size_bytes = EXCLUDED.size_bytes,
                    progress_bytes = EXCLUDED.progress_bytes,
                    progress_ratio = EXCLUDED.progress_ratio,
                    sha256_hex = EXCLUDED.sha256_hex,
                    chunk_size_bytes = EXCLUDED.chunk_size_bytes,
                    last_chunk_size_bytes = EXCLUDED.last_chunk_size_bytes,
                    last_chunk_sha256_hex = EXCLUDED.last_chunk_sha256_hex,
                    rate_limit_kbps = EXCLUDED.rate_limit_kbps,
                    resumed = EXCLUDED.resumed,
                    last_event = EXCLUDED.last_event,
                    last_job_id = EXCLUDED.last_job_id,
                    last_command_type = EXCLUDED.last_command_type,
                    last_seq = EXCLUDED.last_seq,
                    observed_at = EXCLUDED.observed_at,
                    handoff_available = EXCLUDED.handoff_available,
                    handoff_object_key = EXCLUDED.handoff_object_key,
                    handoff_download_path = EXCLUDED.handoff_download_path
                "#,
            )
            .bind(session.session_id)
            .bind(&session.client_id)
            .bind(&session.direction)
            .bind(&session.status)
            .bind(&session.path)
            .bind(session.size_bytes)
            .bind(session.progress_bytes)
            .bind(session.progress_ratio)
            .bind(&session.sha256_hex)
            .bind(session.chunk_size_bytes)
            .bind(session.last_chunk_size_bytes)
            .bind(&session.last_chunk_sha256_hex)
            .bind(session.rate_limit_kbps)
            .bind(session.resumed)
            .bind(&session.last_event)
            .bind(session.last_job_id)
            .bind(&session.last_command_type)
            .bind(session.last_seq)
            .bind(&session.observed_at)
            .bind(session.handoff_available)
            .bind(&session.handoff_object_key)
            .bind(&session.handoff_download_path)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn list_file_transfer_download_handoff_chunks(
        &self,
        client_id: &str,
        session_id: Uuid,
    ) -> Result<Vec<FileTransferDownloadHandoffChunk>> {
        let outputs = self
            .list_file_transfer_download_chunk_outputs(client_id, session_id)
            .await?;
        Ok(build_file_transfer_download_handoff_chunks(
            outputs, session_id,
        ))
    }

    async fn list_file_transfer_download_chunk_outputs(
        &self,
        client_id: &str,
        session_id: Uuid,
    ) -> Result<Vec<FileTransferChunkOutput>> {
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
                        if output.client_id != client_id {
                            return None;
                        }
                        let command_type = command_types.get(&output.job_id)?;
                        if command_type != "file_transfer_download_chunk" {
                            return None;
                        }
                        Some(FileTransferChunkOutput {
                            output: output.clone(),
                        })
                    })
                    .collect::<Vec<_>>();
                outputs.sort_by(|left, right| {
                    left.output
                        .job_id
                        .cmp(&right.output.job_id)
                        .then_with(|| left.output.seq.cmp(&right.output.seq))
                });
                Ok(outputs)
            }
            Self::Postgres(pool) => {
                let session_text = session_id.to_string();
                let rows = sqlx::query(
                    r#"
                    WITH chunk_jobs AS (
                        SELECT DISTINCT output.job_id
                        FROM job_outputs output
                        JOIN jobs job ON job.id = output.job_id
                        WHERE output.client_id = $1
                          AND output.stream = 'status'
                          AND job.command_type = 'file_transfer_download_chunk'
                          AND convert_from(output.data, 'UTF8')::jsonb ->> 'session_id' = $2
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
                        output.created_at::text AS created_at
                    FROM job_outputs output
                    JOIN chunk_jobs ON chunk_jobs.job_id = output.job_id
                    WHERE output.client_id = $1
                    ORDER BY output.job_id, output.seq
                    "#,
                )
                .bind(client_id)
                .bind(session_text)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let data: Vec<u8> = row.try_get("data")?;
                        Ok(FileTransferChunkOutput {
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
                                received_at: None,
                                created_at: row.try_get("created_at")?,
                            },
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }
}

async fn file_transfer_sessions_from_outputs(
    pool: &PgPool,
    client_id: Option<&str>,
    session_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<FileTransferSessionView>> {
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
            'file_transfer_start',
            'file_transfer_chunk',
            'file_transfer_commit',
            'file_transfer_abort',
            'file_transfer_download_start',
            'file_transfer_download_chunk'
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
            Ok(FileTransferStatusOutput {
                job_id: row.try_get("job_id")?,
                client_id: row.try_get("client_id")?,
                seq: row.try_get("seq")?,
                data: row.try_get("data")?,
                created_at: row.try_get("created_at")?,
                command_type: row.try_get("command_type")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
    Ok(build_file_transfer_sessions(outputs, limit, session_id))
}

fn file_transfer_session_from_row(
    row: PgRow,
) -> std::result::Result<FileTransferSessionView, sqlx::Error> {
    Ok(FileTransferSessionView {
        session_id: row.try_get("session_id")?,
        client_id: row.try_get("client_id")?,
        direction: row.try_get("direction")?,
        status: row.try_get("status")?,
        path: row.try_get("path")?,
        size_bytes: row.try_get("size_bytes")?,
        progress_bytes: row.try_get("progress_bytes")?,
        progress_ratio: row.try_get("progress_ratio")?,
        sha256_hex: row.try_get("sha256_hex")?,
        chunk_size_bytes: row.try_get("chunk_size_bytes")?,
        last_chunk_size_bytes: row.try_get("last_chunk_size_bytes")?,
        last_chunk_sha256_hex: row.try_get("last_chunk_sha256_hex")?,
        rate_limit_kbps: row.try_get("rate_limit_kbps")?,
        resumed: row.try_get("resumed")?,
        last_event: row.try_get("last_event")?,
        last_job_id: row.try_get("last_job_id")?,
        last_command_type: row.try_get("last_command_type")?,
        last_seq: row.try_get("last_seq")?,
        observed_at: row.try_get("observed_at")?,
        handoff_available: row.try_get("handoff_available")?,
        handoff_evidence_status: if row.try_get::<bool, _>("handoff_available")? {
            HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE.to_string()
        } else {
            HANDOFF_EVIDENCE_NOT_COMPLETED.to_string()
        },
        handoff_unavailable_reason: None,
        handoff_object_key: row.try_get("handoff_object_key")?,
        handoff_download_path: row.try_get("handoff_download_path")?,
    })
}

#[derive(Clone, Debug)]
struct FileTransferStatusOutput {
    job_id: Uuid,
    client_id: String,
    seq: i32,
    data: Vec<u8>,
    created_at: String,
    command_type: String,
}

#[derive(Clone, Debug)]
struct FileTransferEvent {
    session_id: Uuid,
    client_id: String,
    direction: &'static str,
    status: &'static str,
    path: String,
    size_bytes: Option<i64>,
    progress_bytes: i64,
    sha256_hex: Option<String>,
    chunk_size_bytes: Option<i64>,
    last_chunk_size_bytes: Option<i64>,
    last_chunk_sha256_hex: Option<String>,
    rate_limit_kbps: Option<i64>,
    resumed: Option<bool>,
    event_type: String,
    job_id: Uuid,
    command_type: String,
    seq: i32,
    created_at: String,
}

#[derive(Clone, Debug)]
struct FileTransferChunkOutput {
    output: JobOutputView,
}

#[derive(Clone, Debug)]
pub(crate) struct FileTransferDownloadHandoffChunk {
    pub(crate) job_id: Uuid,
    pub(crate) offset: i64,
    pub(crate) size_bytes: i64,
    pub(crate) sha256_hex: String,
    pub(crate) outputs: Vec<JobOutputView>,
}

#[derive(Clone, Debug)]
struct FileTransferAggregate {
    latest: FileTransferEvent,
    path: String,
    size_bytes: Option<i64>,
    sha256_hex: Option<String>,
    chunk_size_bytes: Option<i64>,
    last_chunk_size_bytes: Option<i64>,
    last_chunk_sha256_hex: Option<String>,
    rate_limit_kbps: Option<i64>,
    resumed: Option<bool>,
}

impl FileTransferAggregate {
    fn new(event: FileTransferEvent) -> Self {
        Self {
            path: event.path.clone(),
            size_bytes: event.size_bytes,
            sha256_hex: event.sha256_hex.clone(),
            chunk_size_bytes: event.chunk_size_bytes,
            last_chunk_size_bytes: event.last_chunk_size_bytes,
            last_chunk_sha256_hex: event.last_chunk_sha256_hex.clone(),
            rate_limit_kbps: event.rate_limit_kbps,
            resumed: event.resumed,
            latest: event,
        }
    }

    fn merge_older(&mut self, event: FileTransferEvent) {
        if self.path.is_empty() {
            self.path = event.path;
        }
        self.size_bytes = self.size_bytes.or(event.size_bytes);
        self.sha256_hex = self.sha256_hex.take().or(event.sha256_hex);
        self.chunk_size_bytes = self.chunk_size_bytes.or(event.chunk_size_bytes);
        self.last_chunk_size_bytes = self.last_chunk_size_bytes.or(event.last_chunk_size_bytes);
        self.last_chunk_sha256_hex = self
            .last_chunk_sha256_hex
            .take()
            .or(event.last_chunk_sha256_hex);
        self.rate_limit_kbps = self.rate_limit_kbps.or(event.rate_limit_kbps);
        self.resumed = self.resumed.or(event.resumed);
    }

    fn into_view(self) -> FileTransferSessionView {
        let progress_ratio = self.size_bytes.and_then(|size| {
            if size > 0 {
                Some((self.latest.progress_bytes as f64 / size as f64).clamp(0.0, 1.0))
            } else {
                None
            }
        });
        let handoff_available = self.latest.direction == "download"
            && self.latest.status == "completed"
            && self.size_bytes.is_some()
            && self.sha256_hex.is_some();
        let handoff_object_key = self.handoff_object_key().filter(|_| handoff_available);
        let handoff_download_path = self.handoff_download_path().filter(|_| handoff_available);
        let (handoff_evidence_status, handoff_unavailable_reason) =
            initial_handoff_evidence(self.latest.direction, self.latest.status, handoff_available);
        FileTransferSessionView {
            session_id: self.latest.session_id,
            client_id: self.latest.client_id,
            direction: self.latest.direction.to_string(),
            status: self.latest.status.to_string(),
            path: self.path,
            size_bytes: self.size_bytes,
            progress_bytes: self.latest.progress_bytes,
            progress_ratio,
            sha256_hex: self.sha256_hex,
            chunk_size_bytes: self.chunk_size_bytes,
            last_chunk_size_bytes: self.last_chunk_size_bytes,
            last_chunk_sha256_hex: self.last_chunk_sha256_hex,
            rate_limit_kbps: self.rate_limit_kbps,
            resumed: self.resumed,
            last_event: self.latest.event_type,
            last_job_id: self.latest.job_id,
            last_command_type: self.latest.command_type,
            last_seq: self.latest.seq,
            observed_at: self.latest.created_at,
            handoff_available,
            handoff_evidence_status,
            handoff_unavailable_reason,
            handoff_object_key,
            handoff_download_path,
        }
    }

    fn handoff_object_key(&self) -> Option<String> {
        let sha256_hex = self.sha256_hex.as_deref()?;
        Some(file_transfer_handoff_object_key(
            &self.latest.client_id,
            self.latest.session_id,
            sha256_hex,
        ))
    }

    fn handoff_download_path(&self) -> Option<String> {
        self.sha256_hex.as_ref()?;
        Some(file_transfer_handoff_download_path(
            &self.latest.client_id,
            self.latest.session_id,
        ))
    }
}

fn build_file_transfer_sessions(
    outputs: Vec<FileTransferStatusOutput>,
    limit: i64,
    session_filter: Option<Uuid>,
) -> Vec<FileTransferSessionView> {
    let mut order = Vec::<(String, Uuid)>::new();
    let mut aggregates = BTreeMap::<(String, Uuid), FileTransferAggregate>::new();

    for output in outputs {
        let Some(event) = parse_file_transfer_event(output) else {
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
            aggregates.insert(key, FileTransferAggregate::new(event));
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

fn parse_file_transfer_event(output: FileTransferStatusOutput) -> Option<FileTransferEvent> {
    let value = serde_json::from_slice::<Value>(&output.data).ok()?;
    let event_type = value.get("type")?.as_str()?.to_string();
    if !is_file_transfer_status_event(&event_type) {
        return None;
    }
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    let extra = value.get("extra").unwrap_or(&Value::Null);
    let direction = if event_type.starts_with("file_transfer_download") {
        "download"
    } else {
        "upload"
    };
    let status = transfer_status(&event_type, extra);
    let size_bytes = value.get("size_bytes").and_then(json_i64);
    let progress_bytes = value
        .get("next_offset")
        .and_then(json_i64)
        .unwrap_or_default()
        .max(0);

    Some(FileTransferEvent {
        session_id,
        client_id: output.client_id,
        direction,
        status,
        path: value
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        size_bytes,
        progress_bytes,
        sha256_hex: first_json_string(extra, &["sha256_hex", "file_sha256_hex"])
            .or_else(|| first_json_string(&value, &["sha256_hex"])),
        chunk_size_bytes: match event_type.as_str() {
            "file_transfer_start" | "file_transfer_download_start" => {
                extra.get("chunk_size_bytes").and_then(json_i64)
            }
            _ => None,
        },
        last_chunk_size_bytes: first_json_i64(extra, &["ack_size_bytes", "chunk_size_bytes"]),
        last_chunk_sha256_hex: first_json_string(extra, &["chunk_sha256_hex"]),
        rate_limit_kbps: extra.get("rate_limit_kbps").and_then(json_i64),
        resumed: extra.get("resumed").and_then(Value::as_bool),
        event_type,
        job_id: output.job_id,
        command_type: output.command_type,
        seq: output.seq,
        created_at: output.created_at,
    })
}

fn transfer_status(event_type: &str, extra: &Value) -> &'static str {
    file_transfer_session_status(
        event_type,
        extra
            .get("complete")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
}

fn first_json_string(value: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .map(ToString::to_string)
}

fn first_json_i64(value: &Value, fields: &[&str]) -> Option<i64> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(json_i64))
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

fn is_file_transfer_command(command_type: &str) -> bool {
    is_file_transfer_command_type(command_type)
}

fn is_file_transfer_status_event(event_type: &str) -> bool {
    is_file_transfer_session_event(event_type)
}

fn build_file_transfer_download_handoff_chunks(
    outputs: Vec<FileTransferChunkOutput>,
    session_id: Uuid,
) -> Vec<FileTransferDownloadHandoffChunk> {
    let mut by_job = BTreeMap::<Uuid, Vec<FileTransferChunkOutput>>::new();
    for output in outputs {
        by_job.entry(output.output.job_id).or_default().push(output);
    }
    let mut chunks = Vec::new();
    for (job_id, outputs) in by_job {
        let Some(status) = outputs.iter().find_map(|output| {
            if output.output.stream != "status" {
                return None;
            }
            parse_download_chunk_status(&output.output, session_id)
        }) else {
            continue;
        };
        let data_outputs = outputs
            .into_iter()
            .filter(|output| output.output.stream == "stdout")
            .map(|output| output.output)
            .collect::<Vec<_>>();
        if data_outputs.is_empty() {
            continue;
        }
        chunks.push(FileTransferDownloadHandoffChunk {
            job_id,
            offset: status.offset,
            size_bytes: status.size_bytes,
            sha256_hex: status.sha256_hex,
            outputs: data_outputs,
        });
    }
    chunks.sort_by(|left, right| {
        left.offset
            .cmp(&right.offset)
            .then_with(|| left.job_id.cmp(&right.job_id))
    });
    chunks
}

#[derive(Clone, Debug)]
struct DownloadChunkStatus {
    offset: i64,
    size_bytes: i64,
    sha256_hex: String,
}

fn parse_download_chunk_status(
    output: &JobOutputView,
    expected_session_id: Uuid,
) -> Option<DownloadChunkStatus> {
    let value = serde_json::from_slice::<Value>(&BASE64.decode(&output.data_base64).ok()?).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("file_transfer_download_chunk") {
        return None;
    }
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    if session_id != expected_session_id {
        return None;
    }
    let extra = value.get("extra")?;
    Some(DownloadChunkStatus {
        offset: extra.get("offset").and_then(json_i64)?,
        size_bytes: first_json_i64(extra, &["chunk_size_bytes"])?,
        sha256_hex: first_json_string(extra, &["chunk_sha256_hex"])?,
    })
}

pub(crate) fn file_transfer_handoff_object_key(
    client_id: &str,
    session_id: Uuid,
    sha256_hex: &str,
) -> String {
    format!(
        "file-transfers/{}/{session_id}/{sha256_hex}.bin",
        hex::encode(client_id.as_bytes())
    )
}

pub(crate) fn file_transfer_handoff_download_path(client_id: &str, session_id: Uuid) -> String {
    format!(
        "/api/v1/file-transfers/{}/{session_id}/handoff/artifact",
        percent_encode_path_segment(client_id)
    )
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

#[derive(Clone, Debug)]
struct HandoffChunkEvidence {
    available: bool,
    status: &'static str,
    reason: Option<String>,
}

impl HandoffChunkEvidence {
    fn available() -> Self {
        Self {
            available: true,
            status: HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE,
            reason: None,
        }
    }

    fn unavailable(status: &'static str, reason: &'static str) -> Self {
        Self {
            available: false,
            status,
            reason: Some(reason.to_string()),
        }
    }
}

#[derive(Clone, Debug)]
struct HandoffOffsetEvidence {
    size_bytes: i64,
    sha256_hex: String,
    output_available: bool,
}

fn reset_and_validate_handoff_session(
    session: &mut FileTransferSessionView,
) -> Option<(String, i64)> {
    if session.direction != "download" {
        set_handoff_evidence(
            session,
            false,
            HANDOFF_EVIDENCE_NOT_APPLICABLE,
            Some("upload_session".to_string()),
            None,
            None,
        );
        return None;
    }
    if session.status != "completed" {
        set_handoff_evidence(
            session,
            false,
            HANDOFF_EVIDENCE_NOT_COMPLETED,
            Some("session_not_completed".to_string()),
            None,
            None,
        );
        return None;
    }
    let Some(size_bytes) = session.size_bytes else {
        set_handoff_evidence(
            session,
            false,
            HANDOFF_EVIDENCE_MISSING_FINAL_METADATA,
            Some("missing_size_bytes".to_string()),
            None,
            None,
        );
        return None;
    };
    let Some(sha256_hex) = session.sha256_hex.clone() else {
        set_handoff_evidence(
            session,
            false,
            HANDOFF_EVIDENCE_MISSING_FINAL_METADATA,
            Some("missing_sha256_hex".to_string()),
            None,
            None,
        );
        return None;
    };
    Some((sha256_hex, size_bytes))
}

fn initial_handoff_evidence(
    direction: &str,
    status: &str,
    basic_available: bool,
) -> (String, Option<String>) {
    if basic_available {
        (
            HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE.to_string(),
            None,
        )
    } else if direction != "download" {
        (
            HANDOFF_EVIDENCE_NOT_APPLICABLE.to_string(),
            Some("upload_session".to_string()),
        )
    } else if status != "completed" {
        (
            HANDOFF_EVIDENCE_NOT_COMPLETED.to_string(),
            Some("session_not_completed".to_string()),
        )
    } else {
        (
            HANDOFF_EVIDENCE_MISSING_FINAL_METADATA.to_string(),
            Some("missing_size_or_hash".to_string()),
        )
    }
}

fn set_handoff_evidence(
    session: &mut FileTransferSessionView,
    available: bool,
    status: &str,
    reason: Option<String>,
    object_key: Option<String>,
    download_path: Option<String>,
) {
    session.handoff_available = available;
    session.handoff_evidence_status = status.to_string();
    session.handoff_unavailable_reason = reason;
    session.handoff_object_key = object_key;
    session.handoff_download_path = download_path;
}

#[cfg(test)]
mod tests {
    use super::{
        build_file_transfer_sessions, file_transfer_handoff_download_path,
        file_transfer_handoff_object_key, FileTransferStatusOutput,
    };
    use uuid::Uuid;

    #[test]
    fn builds_latest_upload_session_with_start_metadata() {
        let session_id = Uuid::new_v4();
        let start_job = Uuid::new_v4();
        let chunk_job = Uuid::new_v4();
        let commit_job = Uuid::new_v4();
        let outputs = vec![
            status_output(
                commit_job,
                "edge-a",
                0,
                "300",
                "file_transfer_commit",
                "file_transfer_commit",
                serde_json::json!({
                    "type": "file_transfer_commit",
                    "session_id": session_id,
                    "path": "/opt/app.bin",
                    "next_offset": 12,
                    "size_bytes": 12,
                    "extra": {"sha256_hex": "b".repeat(64), "mode": 420}
                }),
            ),
            status_output(
                chunk_job,
                "edge-a",
                0,
                "200",
                "file_transfer_chunk",
                "file_transfer_chunk_ack",
                serde_json::json!({
                    "type": "file_transfer_chunk_ack",
                    "session_id": session_id,
                    "path": "/opt/app.bin",
                    "next_offset": 12,
                    "size_bytes": 12,
                    "extra": {"ack_offset": 0, "ack_size_bytes": 12}
                }),
            ),
            status_output(
                start_job,
                "edge-a",
                0,
                "100",
                "file_transfer_start",
                "file_transfer_start",
                serde_json::json!({
                    "type": "file_transfer_start",
                    "session_id": session_id,
                    "path": "/opt/app.bin",
                    "next_offset": 0,
                    "size_bytes": 12,
                    "extra": {"resumed": false, "chunk_size_bytes": 65536, "rate_limit_kbps": 1000}
                }),
            ),
        ];

        let sessions = build_file_transfer_sessions(outputs, 20, None);

        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.session_id, session_id);
        assert_eq!(session.client_id, "edge-a");
        assert_eq!(session.direction, "upload");
        assert_eq!(session.status, "completed");
        assert_eq!(session.path, "/opt/app.bin");
        assert_eq!(session.progress_bytes, 12);
        assert_eq!(session.progress_ratio, Some(1.0));
        assert_eq!(session.chunk_size_bytes, Some(65536));
        assert_eq!(session.last_chunk_size_bytes, Some(12));
        assert_eq!(session.rate_limit_kbps, Some(1000));
        assert_eq!(session.last_job_id, commit_job);
        assert_eq!(session.last_event, "file_transfer_commit");
    }

    #[test]
    fn filters_download_sessions_and_marks_final_chunk_complete() {
        let wanted = Uuid::new_v4();
        let other = Uuid::new_v4();
        let outputs = vec![
            status_output(
                Uuid::new_v4(),
                "edge-b",
                1,
                "300",
                "file_transfer_download_chunk",
                "file_transfer_download_chunk",
                serde_json::json!({
                    "type": "file_transfer_download_chunk",
                    "session_id": wanted,
                    "path": "/var/log/app.log",
                    "next_offset": 100,
                    "size_bytes": 100,
                    "extra": {"offset": 64, "chunk_size_bytes": 36, "chunk_sha256_hex": "a".repeat(64), "complete": true, "file_sha256_hex": "c".repeat(64)}
                }),
            ),
            status_output(
                Uuid::new_v4(),
                "edge-b",
                0,
                "200",
                "file_transfer_download_start",
                "file_transfer_download_start",
                serde_json::json!({
                    "type": "file_transfer_download_start",
                    "session_id": wanted,
                    "path": "/var/log/app.log",
                    "next_offset": 0,
                    "size_bytes": 100,
                    "extra": {"resumed": true, "sha256_hex": "c".repeat(64), "chunk_size_bytes": 64, "rate_limit_kbps": 0}
                }),
            ),
            status_output(
                Uuid::new_v4(),
                "edge-c",
                0,
                "100",
                "file_transfer_download_start",
                "file_transfer_download_start",
                serde_json::json!({
                    "type": "file_transfer_download_start",
                    "session_id": other,
                    "path": "/tmp/other",
                    "next_offset": 0,
                    "size_bytes": 1,
                    "extra": {"chunk_size_bytes": 1}
                }),
            ),
        ];

        let sessions = build_file_transfer_sessions(outputs, 20, Some(wanted));

        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.session_id, wanted);
        assert_eq!(session.direction, "download");
        assert_eq!(session.status, "completed");
        assert_eq!(session.chunk_size_bytes, Some(64));
        assert_eq!(session.last_chunk_size_bytes, Some(36));
        assert_eq!(session.resumed, Some(true));
        let expected_file_hash = "c".repeat(64);
        let expected_chunk_hash = "a".repeat(64);
        assert_eq!(
            session.sha256_hex.as_deref(),
            Some(expected_file_hash.as_str())
        );
        assert_eq!(
            session.last_chunk_sha256_hex.as_deref(),
            Some(expected_chunk_hash.as_str())
        );
        assert!(session.handoff_available);
        assert_eq!(
            session.handoff_object_key.as_deref(),
            Some(file_transfer_handoff_object_key("edge-b", wanted, &expected_file_hash).as_str())
        );
        assert_eq!(
            session.handoff_download_path.as_deref(),
            Some(file_transfer_handoff_download_path("edge-b", wanted).as_str())
        );
    }

    #[test]
    fn handoff_download_path_percent_encodes_client_id() {
        let session_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();

        assert_eq!(
            file_transfer_handoff_download_path("edge a/ignored", session_id),
            "/api/v1/file-transfers/edge%20a%2Fignored/11111111-2222-4333-8444-555555555555/handoff/artifact"
        );
    }

    fn status_output(
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        created_at: &str,
        command_type: &str,
        expected_type: &str,
        value: serde_json::Value,
    ) -> FileTransferStatusOutput {
        assert_eq!(
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap(),
            expected_type
        );
        FileTransferStatusOutput {
            job_id,
            client_id: client_id.to_string(),
            seq,
            data: serde_json::to_vec(&value).unwrap(),
            created_at: created_at.to_string(),
            command_type: command_type.to_string(),
        }
    }
}
