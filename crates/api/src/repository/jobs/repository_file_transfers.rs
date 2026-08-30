use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::Value;
use sqlx::{postgres::PgRow, Postgres, Row, Transaction};
#[cfg(test)]
use std::collections::BTreeSet;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
};
use uuid::Uuid;
use vpsman_common::{file_transfer_session_status, is_file_transfer_session_event};

use crate::{
    model::JobOutputView, model_file_transfer::FileTransferSessionView, repository::Repository,
    repository_key_lifecycle::lock_postgres_client_lifecycles_in_tx,
};

const HANDOFF_EVIDENCE_ARTIFACT_AVAILABLE: &str = "artifact_available";
const HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE: &str = "retained_outputs_available";
const HANDOFF_EVIDENCE_NOT_APPLICABLE: &str = "not_applicable";
const HANDOFF_EVIDENCE_NOT_COMPLETED: &str = "not_completed";
const HANDOFF_EVIDENCE_MISSING_FINAL_METADATA: &str = "missing_final_metadata";
const HANDOFF_EVIDENCE_RETAINED_OUTPUTS_PRUNED: &str = "retained_outputs_pruned";
const HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE: &str = "retained_outputs_incomplete";
const HANDOFF_EVIDENCE_RETAINED_OUTPUTS_CONFLICT: &str = "retained_outputs_conflict";

pub(crate) const FILE_TRANSFER_HANDOFF_EVIDENCE_SQL: &str = r#"
WITH requested AS (
    SELECT *
    FROM unnest(
        $1::bigint[],
        $2::text[],
        $3::uuid[],
        $4::text[],
        $5::bigint[],
        $6::text[]
    ) AS request(
        request_index,
        client_id,
        session_id,
        sha256_hex,
        size_bytes,
        object_key
    )
), evidence_requests AS MATERIALIZED (
    SELECT
        request.*,
        final_artifact.id IS NOT NULL AS final_artifact_available
    FROM requested request
    LEFT JOIN server_artifacts final_artifact
      ON final_artifact.object_key = request.object_key
     AND final_artifact.domain = 'file_transfer_handoff'
     AND final_artifact.sha256_hex = request.sha256_hex
     AND final_artifact.size_bytes = request.size_bytes
     AND final_artifact.status = 'active'
), chunk_jobs AS MATERIALIZED (
    -- The immutable command payload owns session membership. Status output is
    -- still parsed and validated in Rust so malformed evidence stays local to
    -- its job rather than failing the complete batch.
    SELECT request.request_index, request.client_id, job.id AS job_id
    FROM jobs job
    JOIN evidence_requests request
      ON NOT request.final_artifact_available
     AND job.resource_id = request.session_id
    WHERE job.resource_kind = 'file_transfer_session'
      AND job.command_type = 'file_transfer_download_chunk'
), chunk_evidence AS MATERIALIZED (
    -- One row per chunk job prevents large inline output from multiplying the
    -- batch result. Ordered arrays retain the exact per-output validation
    -- semantics without returning stdout bytes to the API process.
    SELECT
        chunk_job.request_index,
        chunk_job.client_id,
        chunk_job.job_id,
        COALESCE(
            array_agg(output.data ORDER BY output.seq)
                FILTER (WHERE output.stream = 'status'),
            ARRAY[]::bytea[]
        ) AS status_outputs,
        COALESCE(
            array_agg(
                CASE output.storage
                    WHEN 'inline' THEN octet_length(output.data)::bigint
                    WHEN 'object_store' THEN COALESCE(output.data_size_bytes, 0)
                    ELSE 0
                END
                ORDER BY output.seq
            ) FILTER (WHERE output.stream = 'stdout'),
            ARRAY[]::bigint[]
        ) AS stdout_sizes,
        COALESCE(
            array_agg(
                CASE output.storage
                    WHEN 'inline' THEN TRUE
                    WHEN 'object_store' THEN
                        output.object_key IS NOT NULL
                        AND output.data_sha256_hex IS NOT NULL
                        AND output.data_size_bytes IS NOT NULL
                        AND output_artifact.id IS NOT NULL
                    ELSE FALSE
                END
                ORDER BY output.seq
            ) FILTER (WHERE output.stream = 'stdout'),
            ARRAY[]::boolean[]
        ) AS stdout_available
    FROM chunk_jobs chunk_job
    LEFT JOIN job_outputs output
      ON output.job_id = chunk_job.job_id
     AND output.client_id = chunk_job.client_id
    LEFT JOIN server_artifacts output_artifact
      ON output.storage = 'object_store'
     AND output_artifact.object_key = output.object_key
     AND output_artifact.domain = 'job_output'
     AND output_artifact.sha256_hex = output.data_sha256_hex
     AND output_artifact.size_bytes = output.data_size_bytes
     AND output_artifact.status = 'active'
    GROUP BY chunk_job.request_index, chunk_job.client_id, chunk_job.job_id
)
SELECT
    request.request_index,
    request.final_artifact_available,
    chunk.job_id AS output_job_id,
    chunk.status_outputs,
    chunk.stdout_sizes,
    chunk.stdout_available
FROM evidence_requests request
LEFT JOIN chunk_evidence chunk
  ON chunk.request_index = request.request_index
 AND chunk.client_id = request.client_id
ORDER BY request.request_index, chunk.job_id
"#;

impl Repository {
    pub(crate) async fn list_file_transfer_sessions(
        &self,
        limit: i64,
        client_id: Option<&str>,
        session_id: Option<Uuid>,
    ) -> Result<Vec<FileTransferSessionView>> {
        let limit = limit.clamp(1, 200);
        match self {
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
        let mut requests = Vec::new();
        for (session_index, session) in sessions.iter_mut().enumerate() {
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
            requests.push(HandoffEvidenceRequest {
                session_index,
                client_id: session.client_id.clone(),
                session_id: session.session_id,
                sha256_hex,
                size_bytes,
                object_key,
                download_path,
            });
        }
        let mut evidence_by_session = self.load_file_transfer_handoff_evidence(&requests).await?;
        ensure!(
            evidence_by_session.len() == requests.len(),
            "file transfer handoff evidence batch was incomplete"
        );
        for request in requests {
            let evidence = evidence_by_session
                .remove(&request.session_index)
                .context("file transfer handoff evidence session was missing")?;
            let session = &mut sessions[request.session_index];
            if evidence.final_artifact_available {
                set_handoff_evidence(
                    session,
                    true,
                    HANDOFF_EVIDENCE_ARTIFACT_AVAILABLE,
                    None,
                    Some(request.object_key),
                    Some(request.download_path),
                );
                continue;
            }
            let chunk_evidence = assess_loaded_handoff_chunk_evidence(
                &evidence.chunks,
                request.session_id,
                request.size_bytes,
            );
            if chunk_evidence.available {
                set_handoff_evidence(
                    session,
                    true,
                    HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE,
                    None,
                    Some(request.object_key),
                    Some(request.download_path),
                );
            } else {
                set_handoff_evidence(
                    session,
                    false,
                    chunk_evidence.status,
                    chunk_evidence.reason,
                    None,
                    None,
                );
            }
        }
        Ok(())
    }

    async fn load_file_transfer_handoff_evidence(
        &self,
        requests: &[HandoffEvidenceRequest],
    ) -> Result<HashMap<usize, LoadedHandoffEvidence>> {
        if requests.is_empty() {
            return Ok(HashMap::new());
        }
        let request_indices = requests
            .iter()
            .map(|request| request.session_index as i64)
            .collect::<Vec<_>>();
        let client_ids = requests
            .iter()
            .map(|request| request.client_id.clone())
            .collect::<Vec<_>>();
        let session_ids = requests
            .iter()
            .map(|request| request.session_id)
            .collect::<Vec<_>>();
        let sha256_hexes = requests
            .iter()
            .map(|request| request.sha256_hex.clone())
            .collect::<Vec<_>>();
        let size_bytes = requests
            .iter()
            .map(|request| request.size_bytes)
            .collect::<Vec<_>>();
        let object_keys = requests
            .iter()
            .map(|request| request.object_key.clone())
            .collect::<Vec<_>>();
        let Self::Postgres(pool) = self;
        let rows = sqlx::query(FILE_TRANSFER_HANDOFF_EVIDENCE_SQL)
            .bind(request_indices)
            .bind(client_ids)
            .bind(session_ids)
            .bind(sha256_hexes)
            .bind(size_bytes)
            .bind(object_keys)
            .fetch_all(pool)
            .await?;
        let mut loaded = HashMap::<usize, LoadedHandoffEvidence>::new();
        for row in rows {
            let request_index = usize::try_from(row.try_get::<i64, _>("request_index")?)
                .context("file transfer handoff evidence index was invalid")?;
            let evidence = loaded.entry(request_index).or_default();
            evidence.final_artifact_available = row.try_get("final_artifact_available")?;
            let Some(_job_id) = row.try_get::<Option<Uuid>, _>("output_job_id")? else {
                continue;
            };
            evidence.chunks.push(LoadedHandoffChunkEvidence {
                status_outputs: row.try_get("status_outputs")?,
                stdout_sizes: row.try_get("stdout_sizes")?,
                stdout_available: row.try_get("stdout_available")?,
            });
        }
        Ok(loaded)
    }

    /// Projects one immutable job-output identity into its exact file-transfer
    /// session. Non-file-transfer and non-status outputs are constant-time
    /// no-ops; no client history is scanned.
    pub(crate) async fn project_file_transfer_session_from_job_output(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
    ) -> Result<()> {
        let Self::Postgres(pool) = self;
        let row = sqlx::query(
            r#"
            SELECT output.stream, output.data,
                   output.created_at::text AS created_at,
                   job.command_type
            FROM job_outputs output
            JOIN jobs job ON job.id = output.job_id
            WHERE output.job_id = $1
              AND output.client_id = $2
              AND output.seq = $3
            "#,
        )
        .bind(job_id)
        .bind(client_id)
        .bind(seq)
        .fetch_optional(pool)
        .await?;
        let Some(row) = row else {
            return Ok(());
        };
        if row.try_get::<String, _>("stream")? != "status" {
            return Ok(());
        }
        let output = FileTransferStatusOutput {
            job_id,
            client_id: client_id.to_string(),
            seq,
            data: row.try_get("data")?,
            created_at: row.try_get("created_at")?,
            command_type: row.try_get("command_type")?,
        };
        let Some(event) = parse_file_transfer_event(output) else {
            return Ok(());
        };
        let incoming = FileTransferAggregate::new(event).into_view();
        let mut tx = pool.begin().await?;
        lock_postgres_client_lifecycles_in_tx(&mut tx, &[client_id.to_string()]).await?;
        // A session may not have a row yet. This short exact-identity lock
        // closes only that first-insert race; producers and unrelated sessions
        // never acquire it.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!(
                "vpsman:file-transfer-session-projection:{client_id}:{}",
                incoming.session_id
            ))
            .execute(&mut *tx)
            .await?;
        let existing = sqlx::query(
            r#"
            SELECT
                session_id, client_id, direction, status, path, size_bytes,
                progress_bytes, progress_ratio, sha256_hex, chunk_size_bytes,
                last_chunk_size_bytes, last_chunk_sha256_hex, rate_limit_kbps,
                resumed, last_event, last_job_id, last_command_type, last_seq,
                observed_at::text AS observed_at, handoff_available,
                handoff_object_key, handoff_download_path
            FROM file_transfer_sessions
            WHERE client_id = $1 AND session_id = $2
            FOR UPDATE
            "#,
        )
        .bind(client_id)
        .bind(incoming.session_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(file_transfer_session_from_row)
        .transpose()?;
        let projected = match existing {
            Some(existing) => merge_persisted_file_transfer_session(existing, incoming)?,
            None => incoming,
        };
        upsert_postgres_file_transfer_session_in_tx(&mut tx, &projected).await?;
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
            Self::Postgres(pool) => {
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
                    JOIN jobs job ON job.id = output.job_id
                    WHERE output.client_id = $1
                      AND job.resource_kind = 'file_transfer_session'
                      AND job.command_type = 'file_transfer_download_chunk'
                      AND job.resource_id = $2
                    ORDER BY output.job_id, output.seq
                    "#,
                )
                .bind(client_id)
                .bind(session_id)
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

async fn upsert_postgres_file_transfer_session_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &FileTransferSessionView,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO file_transfer_sessions (
            session_id, client_id, direction, status, path, size_bytes,
            progress_bytes, progress_ratio, sha256_hex, chunk_size_bytes,
            last_chunk_size_bytes, last_chunk_sha256_hex, rate_limit_kbps,
            resumed, last_event, last_job_id, last_command_type, last_seq,
            observed_at, handoff_available, handoff_object_key,
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
    .execute(&mut **tx)
    .await?;
    Ok(())
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
    Ok(if incoming.last_job_id == existing.last_job_id {
        // Output sequence is the authoritative order inside one immutable
        // job stream, even when transport retry makes an older chunk arrive
        // later in wall-clock time.
        incoming
            .last_seq
            .cmp(&existing.last_seq)
            .then_with(|| incoming_observed_at.cmp(&existing_observed_at))
    } else {
        incoming_observed_at
            .cmp(&existing_observed_at)
            .then_with(|| incoming.last_job_id.cmp(&existing.last_job_id))
            .then_with(|| incoming.last_seq.cmp(&existing.last_seq))
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
struct HandoffEvidenceRequest {
    session_index: usize,
    client_id: String,
    session_id: Uuid,
    sha256_hex: String,
    size_bytes: i64,
    object_key: String,
    download_path: String,
}

#[derive(Clone, Debug, Default)]
struct LoadedHandoffEvidence {
    final_artifact_available: bool,
    chunks: Vec<LoadedHandoffChunkEvidence>,
}

#[derive(Clone, Debug)]
struct LoadedHandoffChunkEvidence {
    status_outputs: Vec<Vec<u8>>,
    stdout_sizes: Vec<i64>,
    stdout_available: Vec<bool>,
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
    rate_limit_kbps: Option<i64>,
    resumed: Option<bool>,
}

impl FileTransferAggregate {
    fn new(event: FileTransferEvent) -> Self {
        Self {
            progress_bytes: event.progress_bytes,
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
    parse_download_chunk_status_data(
        &BASE64.decode(&output.data_base64).ok()?,
        expected_session_id,
    )
}

fn parse_download_chunk_status_data(
    data: &[u8],
    expected_session_id: Uuid,
) -> Option<DownloadChunkStatus> {
    let value = serde_json::from_slice::<Value>(data).ok()?;
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

#[derive(Clone, Debug)]
struct HandoffChunkCandidate {
    offset: i64,
    size_bytes: i64,
    sha256_hex: String,
    output_available: bool,
}

fn assess_loaded_handoff_chunk_evidence(
    chunks: &[LoadedHandoffChunkEvidence],
    expected_session_id: Uuid,
    expected_size_bytes: i64,
) -> HandoffChunkEvidence {
    let candidates = chunks
        .iter()
        .filter_map(|chunk| {
            // The direct path has always ignored a status-only job. Preserve
            // that distinction from a retained chunk whose stdout is present
            // but unavailable.
            if chunk.stdout_sizes.is_empty() {
                return None;
            }
            let status = chunk
                .status_outputs
                .iter()
                .find_map(|data| parse_download_chunk_status_data(data, expected_session_id))?;
            Some(HandoffChunkCandidate {
                offset: status.offset,
                size_bytes: status.size_bytes,
                sha256_hex: status.sha256_hex,
                output_available: summarized_chunk_outputs_available(chunk, status.size_bytes),
            })
        })
        .collect::<Vec<_>>();
    assess_handoff_chunk_candidates(&candidates, expected_size_bytes)
}

fn summarized_chunk_outputs_available(
    chunk: &LoadedHandoffChunkEvidence,
    expected_size_bytes: i64,
) -> bool {
    if chunk.stdout_sizes.is_empty() || chunk.stdout_sizes.len() != chunk.stdout_available.len() {
        return false;
    }
    let mut size_bytes = 0_i64;
    for (part_size, available) in chunk.stdout_sizes.iter().zip(chunk.stdout_available.iter()) {
        if !available {
            return false;
        }
        size_bytes = size_bytes.saturating_add(*part_size);
        if size_bytes > expected_size_bytes {
            return false;
        }
    }
    size_bytes == expected_size_bytes
}

#[cfg(test)]
fn assess_handoff_chunk_evidence(
    chunks: &[FileTransferDownloadHandoffChunk],
    expected_size_bytes: i64,
    active_output_artifacts: &BTreeSet<(Uuid, i32)>,
    inline_output_sizes: &BTreeMap<(Uuid, i32), i64>,
) -> HandoffChunkEvidence {
    let candidates = chunks
        .iter()
        .map(|chunk| HandoffChunkCandidate {
            offset: chunk.offset,
            size_bytes: chunk.size_bytes,
            sha256_hex: chunk.sha256_hex.clone(),
            output_available: handoff_chunk_outputs_available(
                chunk,
                active_output_artifacts,
                inline_output_sizes,
            ),
        })
        .collect::<Vec<_>>();
    assess_handoff_chunk_candidates(&candidates, expected_size_bytes)
}

fn assess_handoff_chunk_candidates(
    chunks: &[HandoffChunkCandidate],
    expected_size_bytes: i64,
) -> HandoffChunkEvidence {
    if expected_size_bytes == 0 && chunks.is_empty() {
        return HandoffChunkEvidence::available();
    }
    if chunks.is_empty() {
        return HandoffChunkEvidence::unavailable(
            HANDOFF_EVIDENCE_RETAINED_OUTPUTS_PRUNED,
            "retained_chunk_outputs_pruned",
        );
    }
    let mut by_offset = BTreeMap::<i64, HandoffOffsetEvidence>::new();
    for chunk in chunks {
        if chunk.offset < 0 || chunk.size_bytes <= 0 {
            return HandoffChunkEvidence::unavailable(
                HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE,
                "chunk_metadata_invalid",
            );
        }
        match by_offset.get_mut(&chunk.offset) {
            Some(existing) => {
                if existing.size_bytes != chunk.size_bytes
                    || existing.sha256_hex != chunk.sha256_hex
                {
                    return HandoffChunkEvidence::unavailable(
                        HANDOFF_EVIDENCE_RETAINED_OUTPUTS_CONFLICT,
                        "duplicate_offset_conflict",
                    );
                }
                existing.output_available |= chunk.output_available;
            }
            None => {
                by_offset.insert(
                    chunk.offset,
                    HandoffOffsetEvidence {
                        size_bytes: chunk.size_bytes,
                        sha256_hex: chunk.sha256_hex.clone(),
                        output_available: chunk.output_available,
                    },
                );
            }
        }
    }
    let mut next_offset = 0_i64;
    for (offset, evidence) in by_offset {
        if offset != next_offset {
            return HandoffChunkEvidence::unavailable(
                HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE,
                "chunk_gap",
            );
        }
        if !evidence.output_available {
            return HandoffChunkEvidence::unavailable(
                HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE,
                "chunk_output_unavailable",
            );
        }
        next_offset = next_offset.saturating_add(evidence.size_bytes);
    }
    if next_offset != expected_size_bytes {
        return HandoffChunkEvidence::unavailable(
            HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE,
            "final_size_mismatch",
        );
    }
    HandoffChunkEvidence::available()
}

#[cfg(test)]
fn handoff_chunk_outputs_available(
    chunk: &FileTransferDownloadHandoffChunk,
    active_output_artifacts: &BTreeSet<(Uuid, i32)>,
    inline_output_sizes: &BTreeMap<(Uuid, i32), i64>,
) -> bool {
    if chunk.outputs.is_empty() {
        return false;
    }
    let mut size_bytes = 0_i64;
    for output in &chunk.outputs {
        match output.storage.as_str() {
            "inline" => {
                let inline_size =
                    if let Some(size) = inline_output_sizes.get(&(output.job_id, output.seq)) {
                        *size
                    } else {
                        let Ok(data) = BASE64.decode(&output.data_base64) else {
                            return false;
                        };
                        data.len() as i64
                    };
                if inline_size < 0 {
                    return false;
                }
                size_bytes = size_bytes.saturating_add(inline_size);
            }
            "object_store" => {
                if output.artifact_object_key.is_none()
                    || output.artifact_sha256_hex.is_none()
                    || output.artifact_size_bytes.is_none()
                    || !active_output_artifacts.contains(&(output.job_id, output.seq))
                {
                    return false;
                }
                size_bytes = size_bytes.saturating_add(
                    output
                        .artifact_size_bytes
                        .expect("object-store output size was checked"),
                );
            }
            _ => return false,
        }
        if size_bytes > chunk.size_bytes {
            return false;
        }
    }
    size_bytes == chunk.size_bytes
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
mod exact_projection_tests {
    use super::{merge_persisted_file_transfer_session, FileTransferSessionView};
    use uuid::Uuid;

    fn session(
        session_id: Uuid,
        job_id: Uuid,
        seq: i32,
        status: &str,
        progress_bytes: i64,
        observed_at: &str,
    ) -> FileTransferSessionView {
        FileTransferSessionView {
            session_id,
            client_id: "edge-a".to_string(),
            direction: "download".to_string(),
            status: status.to_string(),
            path: "/tmp/archive".to_string(),
            size_bytes: Some(100),
            progress_bytes,
            progress_ratio: Some(progress_bytes as f64 / 100.0),
            sha256_hex: Some("hash".to_string()),
            chunk_size_bytes: Some(10),
            last_chunk_size_bytes: None,
            last_chunk_sha256_hex: None,
            rate_limit_kbps: None,
            resumed: None,
            last_event: status.to_string(),
            last_job_id: job_id,
            last_command_type: "file_transfer_download_chunk".to_string(),
            last_seq: seq,
            observed_at: observed_at.to_string(),
            handoff_available: false,
            handoff_evidence_status: "not_completed".to_string(),
            handoff_unavailable_reason: None,
            handoff_object_key: None,
            handoff_download_path: None,
        }
    }

    #[test]
    fn same_job_sequence_not_transport_time_orders_incremental_projection() {
        let session_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        let existing = session(
            session_id,
            job_id,
            1,
            "transferring",
            10,
            "2026-08-28T00:00:10Z",
        );
        let incoming = session(
            session_id,
            job_id,
            2,
            "completed",
            100,
            "2026-08-28T00:00:00Z",
        );

        let merged = merge_persisted_file_transfer_session(existing, incoming).unwrap();
        assert_eq!(merged.last_seq, 2);
        assert_eq!(merged.status, "completed");
        assert_eq!(merged.progress_bytes, 100);
    }
}

#[cfg(test)]
#[path = "tests_repository_file_transfers.rs"]
mod tests;
