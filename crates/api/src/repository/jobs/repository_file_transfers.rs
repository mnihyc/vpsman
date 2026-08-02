use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Row};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
};
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
                sort_file_transfer_outputs_newest(&mut outputs)?;
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
        if sessions.is_empty() {
            return Ok(());
        }
        let session_ids = sessions
            .iter()
            .map(|session| session.session_id)
            .collect::<Vec<_>>();
        let mut tx = pool.begin().await?;
        // Every writer of this derived per-client inventory goes through this
        // refresh path. Serialize refreshes before locking the current rows so
        // a concurrent first insert cannot bypass the same merge invariant.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(client_id)
            .execute(&mut *tx)
            .await?;
        let existing_rows = sqlx::query(
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
            WHERE client_id = $1
              AND session_id = ANY($2)
            FOR UPDATE
            "#,
        )
        .bind(client_id)
        .bind(&session_ids)
        .fetch_all(&mut *tx)
        .await?;
        let mut existing_by_session = existing_rows
            .into_iter()
            .map(|row| {
                let session = file_transfer_session_from_row(row)?;
                Ok((session.session_id, session))
            })
            .collect::<std::result::Result<HashMap<_, _>, sqlx::Error>>()?;
        for incoming in sessions {
            let session = if let Some(existing) = existing_by_session.remove(&incoming.session_id) {
                merge_persisted_file_transfer_session(existing, incoming)?
            } else {
                incoming
            };
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
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
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
                let status_rows = sqlx::query(
                    r#"
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
                    JOIN jobs job ON job.id = output.job_id
                    WHERE output.client_id = $1
                      AND output.stream = 'status'
                      AND job.command_type = 'file_transfer_download_chunk'
                    ORDER BY output.job_id, output.seq
                    "#,
                )
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                let status_outputs = status_rows
                    .into_iter()
                    .map(file_transfer_chunk_output_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
                let chunk_job_ids = status_outputs
                    .iter()
                    .filter(|output| {
                        parse_download_chunk_status(&output.output, session_id).is_some()
                    })
                    .map(|output| output.output.job_id)
                    .collect::<BTreeSet<_>>();
                if chunk_job_ids.is_empty() {
                    return Ok(Vec::new());
                }
                let chunk_job_ids = chunk_job_ids.into_iter().collect::<Vec<_>>();
                let rows = sqlx::query(
                    r#"
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
                    WHERE output.client_id = $1
                      AND output.job_id = ANY($2::uuid[])
                    ORDER BY output.job_id, output.seq
                    "#,
                )
                .bind(client_id)
                .bind(chunk_job_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(file_transfer_chunk_output_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }
}

fn file_transfer_chunk_output_from_row(
    row: PgRow,
) -> std::result::Result<FileTransferChunkOutput, sqlx::Error> {
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

fn merge_persisted_file_transfer_session(
    mut existing: FileTransferSessionView,
    incoming: FileTransferSessionView,
) -> Result<FileTransferSessionView> {
    ensure!(
        existing.client_id == incoming.client_id && existing.session_id == incoming.session_id,
        "file transfer session merge identity mismatch"
    );
    let source_order = compare_file_transfer_sources(&incoming, &existing)?;
    let incoming_source_is_newer = source_order != Ordering::Less;
    let existing_progress = existing.progress_bytes;
    let incoming_progress = incoming.progress_bytes;
    let existing_is_terminal = matches!(existing.status.as_str(), "completed" | "aborted");
    let incoming_is_terminal = matches!(incoming.status.as_str(), "completed" | "aborted");
    let preserve_existing_lifecycle = (existing_is_terminal
        && (!incoming_is_terminal || !incoming_source_is_newer))
        || (!incoming_is_terminal && incoming_progress < existing_progress);

    if incoming_source_is_newer {
        if !incoming.path.is_empty() {
            existing.path = incoming.path.clone();
        }
        existing.size_bytes = incoming.size_bytes.or(existing.size_bytes);
        existing.sha256_hex = incoming.sha256_hex.clone().or(existing.sha256_hex.take());
        existing.chunk_size_bytes = incoming.chunk_size_bytes.or(existing.chunk_size_bytes);
        existing.rate_limit_kbps = incoming.rate_limit_kbps.or(existing.rate_limit_kbps);
        existing.resumed = incoming.resumed.or(existing.resumed);
    } else {
        if existing.path.is_empty() && !incoming.path.is_empty() {
            existing.path = incoming.path.clone();
        }
        existing.size_bytes = existing.size_bytes.or(incoming.size_bytes);
        existing.sha256_hex = existing.sha256_hex.take().or(incoming.sha256_hex.clone());
        existing.chunk_size_bytes = existing.chunk_size_bytes.or(incoming.chunk_size_bytes);
        existing.rate_limit_kbps = existing.rate_limit_kbps.or(incoming.rate_limit_kbps);
        existing.resumed = existing.resumed.or(incoming.resumed);
    }

    match incoming_progress.cmp(&existing_progress) {
        Ordering::Greater => {
            existing.last_chunk_size_bytes = incoming
                .last_chunk_size_bytes
                .or(existing.last_chunk_size_bytes);
            existing.last_chunk_sha256_hex = incoming
                .last_chunk_sha256_hex
                .clone()
                .or(existing.last_chunk_sha256_hex.take());
        }
        Ordering::Equal if incoming_source_is_newer => {
            existing.last_chunk_size_bytes = incoming
                .last_chunk_size_bytes
                .or(existing.last_chunk_size_bytes);
            existing.last_chunk_sha256_hex = incoming
                .last_chunk_sha256_hex
                .clone()
                .or(existing.last_chunk_sha256_hex.take());
        }
        Ordering::Equal => {
            existing.last_chunk_size_bytes = existing
                .last_chunk_size_bytes
                .or(incoming.last_chunk_size_bytes);
            existing.last_chunk_sha256_hex = existing
                .last_chunk_sha256_hex
                .take()
                .or(incoming.last_chunk_sha256_hex.clone());
        }
        Ordering::Less => {}
    }
    existing.progress_bytes = existing_progress.max(incoming_progress);
    existing.progress_ratio = existing.size_bytes.and_then(|size| {
        (size > 0).then(|| (existing.progress_bytes as f64 / size as f64).clamp(0.0, 1.0))
    });

    if !preserve_existing_lifecycle {
        existing.direction = incoming.direction;
        existing.status = incoming.status;
        existing.last_event = incoming.last_event;
        existing.last_job_id = incoming.last_job_id;
        existing.last_command_type = incoming.last_command_type;
        existing.last_seq = incoming.last_seq;
        existing.observed_at = incoming.observed_at;
    }

    let handoff_available = existing.direction == "download"
        && existing.status == "completed"
        && existing.size_bytes.is_some()
        && existing.sha256_hex.is_some();
    existing.handoff_available = handoff_available;
    existing.handoff_object_key = existing
        .sha256_hex
        .as_deref()
        .filter(|_| handoff_available)
        .map(|sha256_hex| {
            file_transfer_handoff_object_key(&existing.client_id, existing.session_id, sha256_hex)
        });
    existing.handoff_download_path = handoff_available
        .then(|| file_transfer_handoff_download_path(&existing.client_id, existing.session_id));
    let (evidence_status, unavailable_reason) =
        initial_handoff_evidence(&existing.direction, &existing.status, handoff_available);
    existing.handoff_evidence_status = evidence_status.to_string();
    existing.handoff_unavailable_reason = unavailable_reason;
    Ok(existing)
}

fn compare_file_transfer_sources(
    incoming: &FileTransferSessionView,
    existing: &FileTransferSessionView,
) -> Result<Ordering> {
    let incoming_observed_at = crate::util::parse_timestamp_utc(&incoming.observed_at)
        .context("incoming file transfer timestamp is invalid")?;
    let existing_observed_at = crate::util::parse_timestamp_utc(&existing.observed_at)
        .context("stored file transfer timestamp is invalid")?;
    Ok(incoming_observed_at
        .cmp(&existing_observed_at)
        .then_with(|| incoming.last_job_id.cmp(&existing.last_job_id))
        .then_with(|| incoming.last_seq.cmp(&existing.last_seq)))
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
    progress_bytes: i64,
    path: String,
    size_bytes: Option<i64>,
    sha256_hex: Option<String>,
    chunk_size_bytes: Option<i64>,
    last_chunk_size_bytes: Option<i64>,
    last_chunk_sha256_hex: Option<String>,
    last_chunk_progress_bytes: Option<i64>,
    rate_limit_kbps: Option<i64>,
    resumed: Option<bool>,
}

impl FileTransferAggregate {
    fn new(event: FileTransferEvent) -> Self {
        let last_chunk_progress_bytes = (event.last_chunk_size_bytes.is_some()
            || event.last_chunk_sha256_hex.is_some())
        .then_some(event.progress_bytes);
        Self {
            progress_bytes: event.progress_bytes,
            path: event.path.clone(),
            size_bytes: event.size_bytes,
            sha256_hex: event.sha256_hex.clone(),
            chunk_size_bytes: event.chunk_size_bytes,
            last_chunk_size_bytes: event.last_chunk_size_bytes,
            last_chunk_sha256_hex: event.last_chunk_sha256_hex.clone(),
            last_chunk_progress_bytes,
            rate_limit_kbps: event.rate_limit_kbps,
            resumed: event.resumed,
            latest: event,
        }
    }

    fn merge_older(&mut self, event: FileTransferEvent) {
        let replacement =
            file_transfer_event_advances_lifecycle(&self.latest, &event).then(|| event.clone());
        if self.path.is_empty() {
            self.path = event.path.clone();
        }
        self.size_bytes = self.size_bytes.or(event.size_bytes);
        self.sha256_hex = self.sha256_hex.take().or(event.sha256_hex.clone());
        self.chunk_size_bytes = self.chunk_size_bytes.or(event.chunk_size_bytes);
        if event.last_chunk_size_bytes.is_some() || event.last_chunk_sha256_hex.is_some() {
            match self.last_chunk_progress_bytes {
                Some(progress) if event.progress_bytes == progress => {
                    self.last_chunk_size_bytes =
                        self.last_chunk_size_bytes.or(event.last_chunk_size_bytes);
                    self.last_chunk_sha256_hex = self
                        .last_chunk_sha256_hex
                        .take()
                        .or(event.last_chunk_sha256_hex.clone());
                }
                None => {
                    self.last_chunk_size_bytes = event.last_chunk_size_bytes;
                    self.last_chunk_sha256_hex = event.last_chunk_sha256_hex.clone();
                    self.last_chunk_progress_bytes = Some(event.progress_bytes);
                }
                Some(progress) if event.progress_bytes > progress => {
                    self.last_chunk_size_bytes = event.last_chunk_size_bytes;
                    self.last_chunk_sha256_hex = event.last_chunk_sha256_hex.clone();
                    self.last_chunk_progress_bytes = Some(event.progress_bytes);
                }
                Some(_) => {}
            }
        }
        self.rate_limit_kbps = self.rate_limit_kbps.or(event.rate_limit_kbps);
        self.resumed = self.resumed.or(event.resumed);
        self.progress_bytes = self.progress_bytes.max(event.progress_bytes);
        if let Some(replacement) = replacement {
            self.latest = replacement;
        }
    }

    fn into_view(self) -> FileTransferSessionView {
        let progress_ratio = self.size_bytes.and_then(|size| {
            if size > 0 {
                Some((self.progress_bytes as f64 / size as f64).clamp(0.0, 1.0))
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
            progress_bytes: self.progress_bytes,
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

fn file_transfer_event_advances_lifecycle(
    current: &FileTransferEvent,
    candidate: &FileTransferEvent,
) -> bool {
    if matches!(current.status, "completed" | "aborted") {
        return false;
    }
    matches!(candidate.status, "completed" | "aborted")
        || candidate.progress_bytes > current.progress_bytes
}

fn sort_file_transfer_outputs_newest(outputs: &mut [FileTransferStatusOutput]) -> Result<()> {
    for output in outputs.iter() {
        crate::util::parse_timestamp_utc(&output.created_at)
            .context("file transfer source timestamp is invalid")?;
    }
    outputs.sort_by(|left, right| {
        crate::util::parse_timestamp_utc(&right.created_at)
            .expect("file transfer timestamps were validated before sorting")
            .cmp(
                &crate::util::parse_timestamp_utc(&left.created_at)
                    .expect("file transfer timestamps were validated before sorting"),
            )
            .then_with(|| right.job_id.cmp(&left.job_id))
            .then_with(|| right.seq.cmp(&left.seq))
    });
    Ok(())
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
#[path = "tests_repository_file_transfers.rs"]
mod tests;
