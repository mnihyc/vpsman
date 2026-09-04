use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use sqlx::{
    postgres::{PgListener, PgRow},
    PgPool, Postgres, Row, Transaction,
};
use std::{collections::BTreeSet, time::Duration};
use tokio::sync::oneshot;
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{payload_hash, CommandOutput, OutputStream};
use vpsman_server_core::{
    target_status_is_active, INLINE_OUTPUT_PREVIEW_BYTES, STATUS_OUTPUT_MAX_BYTES,
};

use crate::model::{
    JobOutputListItemView, JobOutputView, NewServerArtifact, ProcessSupervisorInventoryView,
};
use crate::object_store::BackupObjectStore;
use crate::repository::Repository;
use crate::repository_jobs::{
    enqueue_target_terminal_event_in_tx, finish_jobs_in_tx_and_reconcile_event_sources,
    insert_agent_update_lifecycle_for_stored_job_in_tx,
};
use crate::{output_stream_name, TargetDispatchOutcome};

const JOB_OUTPUT_ARTIFACT_PREFIX: &str = "job-outputs";
const PROCESS_SUPERVISOR_INVENTORY_PAGE_SIZE: i64 = 500;
const PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT: usize = 10_000;
pub(crate) const PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR: &str =
    "process_supervisor_inventory_scan_limit_exceeded";
const JOB_OUTPUT_PROJECTION_CHANNEL: &str = "vpsman_job_output_projection";
const JOB_OUTPUT_PROJECTION_LEASE_SECS: i32 = 30;
const JOB_OUTPUT_PROJECTION_RENEW_SECS: u64 = 10;
const JOB_OUTPUT_PROJECTION_RECOVERY_POLL_SECS: u64 = 5;
const JOB_OUTPUT_PROJECTION_ERROR_RETRY_SECS: i32 = 5;

#[derive(Clone, Copy)]
pub(crate) struct JobOutputPersistConfig<'a> {
    pub(crate) object_store: Option<&'a BackupObjectStore>,
    pub(crate) artifact_min_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JobOutputWriteResult {
    Inserted,
    DuplicateIdentical,
    DuplicateConflict,
}

pub(crate) struct FinalJobOutputRecordResult {
    pub(crate) write_result: JobOutputWriteResult,
    pub(crate) terminal_reconciliation_ready: bool,
}

#[derive(Debug)]
pub(crate) struct ActiveJobOutputChunkRecordResult {
    pub(crate) write_result: JobOutputWriteResult,
    pub(crate) contiguous_final: Option<PendingFinalJobOutput>,
    pub(crate) terminal_reconciliation_ready: bool,
}

#[derive(Debug)]
pub(crate) struct PendingFinalJobOutput {
    pub(crate) seq: i32,
    pub(crate) output: CommandOutput,
    pub(crate) received_at: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ClaimedNetworkTrafficImportFinalization {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) final_seq: i32,
    pub(crate) target_active: bool,
    lease_id: Uuid,
    pool: PgPool,
}

#[derive(Clone)]
pub(crate) struct ClaimedJobOutputProjectionWork {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) seq: i32,
    lease_id: Uuid,
    pool: PgPool,
}

impl ClaimedJobOutputProjectionWork {
    async fn renew(&self) -> Result<bool> {
        let updated = sqlx::query(
            r#"
            UPDATE job_output_projection_work
            SET lease_until = now() + ($5::int * interval '1 second'),
                updated_at = now()
            WHERE job_id = $1 AND client_id = $2 AND seq = $3
              AND lease_id = $4
              AND lease_until > now()
            "#,
        )
        .bind(self.job_id)
        .bind(&self.client_id)
        .bind(self.seq)
        .bind(self.lease_id)
        .bind(JOB_OUTPUT_PROJECTION_LEASE_SECS)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    async fn acknowledge(self) -> Result<bool> {
        let deleted = sqlx::query(
            r#"
            DELETE FROM job_output_projection_work
            WHERE job_id = $1 AND client_id = $2 AND seq = $3
              AND lease_id = $4
            "#,
        )
        .bind(self.job_id)
        .bind(&self.client_id)
        .bind(self.seq)
        .bind(self.lease_id)
        .execute(&self.pool)
        .await?;
        Ok(deleted.rows_affected() == 1)
    }

    async fn defer(self, error: &str) -> Result<bool> {
        let updated = sqlx::query(
            r#"
            UPDATE job_output_projection_work
            SET next_attempt_at = now() + ($5::int * interval '1 second'),
                last_error = left($4, 1000),
                lease_id = NULL,
                lease_until = NULL,
                updated_at = now()
            WHERE job_id = $1 AND client_id = $2 AND seq = $3
              AND lease_id = $6
            "#,
        )
        .bind(self.job_id)
        .bind(&self.client_id)
        .bind(self.seq)
        .bind(error)
        .bind(JOB_OUTPUT_PROJECTION_ERROR_RETRY_SECS)
        .bind(self.lease_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }
}

impl ClaimedNetworkTrafficImportFinalization {
    pub(crate) async fn renew(&self, lease_secs: u64) -> Result<bool> {
        let lease_secs = lease_secs.clamp(1, 3_600);
        let updated = sqlx::query(
            r#"
            UPDATE network_traffic_import_finalizations
            SET lease_until = now() + make_interval(secs => $5::integer),
                updated_at = now()
            WHERE job_id = $1 AND client_id = $2 AND final_seq = $3
              AND lease_id = $4
            "#,
        )
        .bind(self.job_id)
        .bind(&self.client_id)
        .bind(self.final_seq)
        .bind(self.lease_id)
        .bind(i32::try_from(lease_secs).unwrap_or(3_600))
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn acknowledge(self) -> Result<bool> {
        let deleted = sqlx::query(
            r#"
            DELETE FROM network_traffic_import_finalizations
            WHERE job_id = $1 AND client_id = $2 AND final_seq = $3
              AND lease_id = $4
            "#,
        )
        .bind(self.job_id)
        .bind(&self.client_id)
        .bind(self.final_seq)
        .bind(self.lease_id)
        .execute(&self.pool)
        .await?;
        Ok(deleted.rows_affected() == 1)
    }

    pub(crate) async fn defer(self, message: &str, retry_after_secs: u64) -> Result<bool> {
        let retry_after_secs = retry_after_secs.clamp(1, 3_600);
        let updated = sqlx::query(
            r#"
            UPDATE network_traffic_import_finalizations
            SET next_attempt_at = now() + make_interval(secs => $4::integer),
                last_error = left($5, 1000),
                lease_id = NULL,
                lease_until = NULL,
                updated_at = now()
            WHERE job_id = $1 AND client_id = $2 AND final_seq = $3
              AND lease_id = $6
            "#,
        )
        .bind(self.job_id)
        .bind(&self.client_id)
        .bind(self.final_seq)
        .bind(i32::try_from(retry_after_secs).unwrap_or(3_600))
        .bind(message)
        .bind(self.lease_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct JobOutputArtifactRef {
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct JobOutputListFilter {
    pub(crate) client_id: Option<String>,
    pub(crate) stream: Option<String>,
    pub(crate) seq_after: Option<i32>,
    pub(crate) cursor: Option<JobOutputCursor>,
    pub(crate) include_data: bool,
    pub(crate) limit: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct JobOutputCursor {
    pub(crate) client_id: String,
    pub(crate) seq: i32,
}

/// Starts the sole API-owned durable output projector. PostgreSQL NOTIFY is
/// only a wake hint; the indexed work table is drained before every wait and
/// after listener recovery.
pub(crate) fn spawn_job_output_projection_consumer(
    repo: Repository,
) -> tokio::task::JoinHandle<()> {
    let pool = match &repo {
        Repository::Postgres(pool) => pool.clone(),
    };
    tokio::spawn(async move {
        loop {
            let mut listener = match PgListener::connect_with(&pool).await {
                Ok(listener) => listener,
                Err(error) => {
                    warn!(%error, "job-output projection listener connection failed");
                    tokio::time::sleep(Duration::from_secs(
                        JOB_OUTPUT_PROJECTION_RECOVERY_POLL_SECS,
                    ))
                    .await;
                    continue;
                }
            };
            if let Err(error) = listener.listen(JOB_OUTPUT_PROJECTION_CHANNEL).await {
                warn!(%error, "job-output projection listener registration failed");
                tokio::time::sleep(Duration::from_secs(
                    JOB_OUTPUT_PROJECTION_RECOVERY_POLL_SECS,
                ))
                .await;
                continue;
            }
            loop {
                match process_next_job_output_projection(&repo).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => warn!(%error, "durable job-output projection drain failed"),
                }
                match tokio::time::timeout(
                    Duration::from_secs(JOB_OUTPUT_PROJECTION_RECOVERY_POLL_SECS),
                    listener.recv(),
                )
                .await
                {
                    Ok(Ok(_)) | Err(_) => {}
                    Ok(Err(error)) => {
                        warn!(%error, "job-output projection listener disconnected");
                        break;
                    }
                }
            }
        }
    })
}

async fn process_next_job_output_projection(repo: &Repository) -> Result<bool> {
    let Some(owner) = repo.claim_job_output_projection_work().await? else {
        return Ok(false);
    };
    let heartbeat_owner = owner.clone();
    let (heartbeat_stop, heartbeat_stop_rx) = oneshot::channel();
    let heartbeat = tokio::spawn(renew_job_output_projection_owner_until_stopped(
        heartbeat_owner,
        heartbeat_stop_rx,
    ));
    let projection = repo
        .project_persisted_job_output_identity(owner.job_id, &owner.client_id, owner.seq)
        .await;
    let _ = heartbeat_stop.send(());
    let ownership_current = heartbeat
        .await
        .context("job-output projection heartbeat task failed")??;
    if !ownership_current {
        warn!(
            job_id = %owner.job_id,
            client_id = owner.client_id,
            seq = owner.seq,
            "job-output projection ownership changed before completion"
        );
        return Ok(true);
    }
    match projection {
        Ok(()) => {
            if !owner.acknowledge().await? {
                warn!("job-output projection changed owner before acknowledgement");
            }
        }
        Err(error) => {
            let message = error.to_string();
            if !owner.defer(&message).await? {
                warn!(%error, "job-output projection changed owner before deferral");
            } else {
                warn!(%error, "job-output projection deferred after exact projection failure");
            }
        }
    }
    Ok(true)
}

// Full-workflow tests drive the same durable owner as production, but without
// starting a listener task whose scheduling would make assertions racy.
#[cfg(test)]
pub(crate) async fn drain_job_output_projections_for_test(repo: &Repository) -> Result<usize> {
    let mut projected = 0_usize;
    while process_next_job_output_projection(repo).await? {
        projected = projected.saturating_add(1);
    }
    Ok(projected)
}

async fn renew_job_output_projection_owner_until_stopped(
    owner: ClaimedJobOutputProjectionWork,
    mut stop: oneshot::Receiver<()>,
) -> Result<bool> {
    loop {
        tokio::select! {
            _ = &mut stop => return Ok(true),
            _ = tokio::time::sleep(Duration::from_secs(
                JOB_OUTPUT_PROJECTION_RENEW_SECS,
            )) => {
                if !owner.renew().await? {
                    return Ok(false);
                }
            }
        }
    }
}

impl Repository {
    pub(crate) async fn claim_job_output_projection_work(
        &self,
    ) -> Result<Option<ClaimedJobOutputProjectionWork>> {
        let Self::Postgres(pool) = self;
        let lease_id = Uuid::new_v4();
        let row = sqlx::query(
            r#"
            WITH candidate AS MATERIALIZED (
                SELECT job_id, client_id, seq
                FROM job_output_projection_work
                WHERE next_attempt_at <= now()
                  AND (lease_id IS NULL OR lease_until <= now())
                ORDER BY next_attempt_at, created_at, job_id, client_id, seq
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE job_output_projection_work work
            SET attempt_count = work.attempt_count + 1,
                lease_id = $1,
                lease_until = now() + ($2::int * interval '1 second'),
                last_error = NULL,
                updated_at = now()
            FROM candidate
            WHERE work.job_id = candidate.job_id
              AND work.client_id = candidate.client_id
              AND work.seq = candidate.seq
            RETURNING work.job_id, work.client_id, work.seq
            "#,
        )
        .bind(lease_id)
        .bind(JOB_OUTPUT_PROJECTION_LEASE_SECS)
        .fetch_optional(pool)
        .await?;
        row.map(|row| {
            Ok(ClaimedJobOutputProjectionWork {
                job_id: row.try_get("job_id")?,
                client_id: row.try_get("client_id")?,
                seq: row.try_get("seq")?,
                lease_id,
                pool: pool.clone(),
            })
        })
        .transpose()
    }

    pub(crate) async fn claim_network_traffic_import_finalization(
        &self,
        lease_secs: u64,
    ) -> Result<Option<ClaimedNetworkTrafficImportFinalization>> {
        let lease_secs = lease_secs.clamp(1, 3_600);
        match self {
            Self::Postgres(pool) => {
                let lease_id = Uuid::new_v4();
                let row = sqlx::query(
                    r#"
                        WITH candidate AS MATERIALIZED (
                            SELECT
                                finalization.job_id,
                                finalization.client_id,
                                finalization.final_seq,
                                (
                                    target.completed_at IS NULL
                                    AND target.status IN ('dispatching', 'running')
                                ) AS target_active
                            FROM network_traffic_import_finalizations finalization
                            JOIN job_targets target
                              ON target.job_id = finalization.job_id
                             AND target.client_id = finalization.client_id
                            WHERE finalization.next_attempt_at <= now()
                              AND (
                                  finalization.lease_id IS NULL
                                  OR finalization.lease_until <= now()
                              )
                            ORDER BY
                                finalization.next_attempt_at,
                                finalization.created_at,
                                finalization.job_id,
                                finalization.client_id
                            LIMIT 1
                            FOR UPDATE OF finalization SKIP LOCKED
                        )
                        UPDATE network_traffic_import_finalizations finalization
                        SET attempt_count = finalization.attempt_count + 1,
                            lease_id = $1,
                            lease_until = now() + make_interval(secs => $2::integer),
                            last_error = NULL,
                            updated_at = now()
                        FROM candidate
                        WHERE finalization.job_id = candidate.job_id
                          AND finalization.client_id = candidate.client_id
                        RETURNING
                            finalization.job_id,
                            finalization.client_id,
                            finalization.final_seq,
                            candidate.target_active
                        "#,
                )
                .bind(lease_id)
                .bind(i32::try_from(lease_secs).unwrap_or(3_600))
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                Ok(Some(ClaimedNetworkTrafficImportFinalization {
                    job_id: row.try_get("job_id")?,
                    client_id: row.try_get("client_id")?,
                    final_seq: row.try_get("final_seq")?,
                    target_active: row.try_get("target_active")?,
                    lease_id,
                    pool: pool.clone(),
                }))
            }
        }
    }

    pub(crate) async fn list_job_outputs(&self, job_id: Uuid) -> Result<Vec<JobOutputView>> {
        match self {
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
                        received_at::text AS received_at,
                        exit_code,
                        done,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE job_id = $1
                    ORDER BY client_id, seq
                    "#,
                )
                .bind(job_id)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(job_output_view_from_row)
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            }
        }
    }

    pub(crate) async fn list_job_outputs_for_target(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<Vec<JobOutputView>> {
        match self {
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
                        received_at::text AS received_at,
                        exit_code,
                        done,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE job_id = $1
                      AND client_id = $2
                    ORDER BY seq
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(job_output_view_from_row)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(Into::into)
            }
        }
    }

    pub(crate) async fn terminal_result_status_output_for_target(
        &self,
        job_id: Uuid,
        client_id: &str,
        terminal_status: &str,
    ) -> Result<Option<CommandOutput>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
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
                        output.received_at::text AS received_at,
                        output.exit_code,
                        output.done,
                        output.created_at::text AS created_at
                    FROM audit_logs audit
                    JOIN job_outputs output
                      ON output.job_id = $1
                     AND output.client_id = $2
                     AND output.seq = CASE
                            WHEN audit.metadata->>'output_seq' ~ '^[0-9]+$'
                            THEN (audit.metadata->>'output_seq')::integer
                            ELSE NULL
                         END
                    WHERE audit.action = 'job.target_result'
                      AND audit.target = 'client:' || $2
                      AND audit.metadata->>'job_id' = $1::text
                      AND audit.metadata->>'status' = $3
                      AND audit.metadata->>'component' = 'gateway-command-output-ingest'
                      AND output.done
                      AND output.stream = 'status'
                    ORDER BY audit.created_at DESC, audit.id DESC
                    LIMIT 1
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(terminal_status)
                .fetch_optional(pool)
                .await?;
                row.map(job_output_view_from_row)
                    .transpose()?
                    .as_ref()
                    .map(command_output_from_view)
                    .transpose()
            }
        }
    }

    pub(crate) async fn list_job_outputs_page(
        &self,
        job_id: Uuid,
        filter: JobOutputListFilter,
    ) -> Result<Vec<JobOutputListItemView>> {
        let limit = filter.limit.clamp(1, 1001);
        match self {
            Self::Postgres(pool) => {
                let cursor_client_id = filter
                    .cursor
                    .as_ref()
                    .map(|cursor| cursor.client_id.clone());
                let cursor_seq = filter.cursor.as_ref().map(|cursor| cursor.seq);
                let rows = sqlx::query(
                    r#"
                    SELECT
                        job_id,
                        client_id,
                        seq,
                        stream,
                        CASE WHEN $7 THEN data ELSE NULL END AS data,
                        storage,
                        object_key,
                        data_sha256_hex,
                        data_size_bytes,
                        received_at::text AS received_at,
                        exit_code,
                        done,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE job_id = $1
                      AND ($2::text IS NULL OR client_id = $2)
                      AND ($3::text IS NULL OR stream = $3)
                      AND ($4::integer IS NULL OR seq > $4)
                      AND (
                        $5::text IS NULL
                        OR client_id > $5
                        OR (client_id = $5 AND seq > $6)
                      )
                    ORDER BY client_id, seq
                    LIMIT $8
                    "#,
                )
                .bind(job_id)
                .bind(&filter.client_id)
                .bind(&filter.stream)
                .bind(filter.seq_after)
                .bind(cursor_client_id)
                .bind(cursor_seq)
                .bind(filter.include_data)
                .bind(limit)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(job_output_list_item_from_row)
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            }
        }
    }

    pub(crate) async fn get_job_output(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
    ) -> Result<Option<JobOutputView>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
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
                        received_at::text AS received_at,
                        exit_code,
                        done,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE job_id = $1 AND client_id = $2 AND seq = $3
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(seq)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(job_output_view_from_row).transpose()?)
            }
        }
    }

    pub(crate) async fn get_job_output_artifact_ref(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
    ) -> Result<Option<JobOutputArtifactRef>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT object_key, data_sha256_hex, data_size_bytes
                    FROM job_outputs
                    WHERE job_id = $1 AND client_id = $2 AND seq = $3
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(seq)
                .fetch_optional(pool)
                .await?;
                row.map(|row| {
                    let object_key: Option<String> = row.try_get("object_key")?;
                    let sha256_hex: Option<String> = row.try_get("data_sha256_hex")?;
                    let size_bytes: Option<i64> = row.try_get("data_size_bytes")?;
                    Ok(match (object_key, sha256_hex, size_bytes) {
                        (Some(object_key), Some(sha256_hex), Some(size_bytes)) => {
                            Some(JobOutputArtifactRef {
                                object_key,
                                sha256_hex,
                                size_bytes,
                            })
                        }
                        _ => None,
                    })
                })
                .transpose()
                .map(Option::flatten)
            }
        }
    }

    async fn project_persisted_job_output_identity(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
    ) -> Result<()> {
        let Some(output) = self.get_job_output(job_id, client_id, seq).await? else {
            // A cascading job deletion may retire the source and its work row
            // after the claim transaction commits. There is then no surviving
            // authority to project.
            return Ok(());
        };
        if let Some(artifact) = job_output_view_server_artifact(&output) {
            self.register_server_artifact(artifact).await?;
        }
        let command = command_output_from_view(&output)?;
        let observed_at = output
            .received_at
            .clone()
            .unwrap_or_else(|| output.created_at.clone());
        self.record_persisted_network_observations(
            job_id,
            client_id,
            &[(seq, command, observed_at)],
        )
        .await?;
        self.project_file_transfer_session_from_job_output(job_id, client_id, seq)
            .await?;
        self.project_terminal_session_from_job_output(job_id, client_id, seq)
            .await?;
        self.project_terminal_command_replay_from_job_output(&output)
            .await?;
        Ok(())
    }

    pub(crate) async fn list_process_supervisor_inventory(
        &self,
        limit: i64,
    ) -> Result<Vec<ProcessSupervisorInventoryView>> {
        match self {
            Self::Postgres(pool) => {
                let wanted = limit.clamp(1, 200) as usize;
                let mut tx = pool.begin().await?;
                sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                    .execute(&mut *tx)
                    .await?;
                let mut inventory = Vec::new();
                let mut seen = BTreeSet::new();
                let mut after_created_at: Option<String> = None;
                let mut after_job_id: Option<Uuid> = None;
                let mut after_client_id: Option<String> = None;
                let mut after_seq: Option<i32> = None;
                let mut scanned = 0_usize;
                let mut history_exhausted = false;
                while scanned < PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT {
                    let remaining = PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT - scanned;
                    let fetch_limit =
                        if remaining <= PROCESS_SUPERVISOR_INVENTORY_PAGE_SIZE as usize {
                            remaining + 1
                        } else {
                            PROCESS_SUPERVISOR_INVENTORY_PAGE_SIZE as usize
                        };
                    let rows = sqlx::query(
                        r#"
                        SELECT
                            output.job_id,
                            output.client_id,
                            output.seq,
                            output.stream,
                            output.data,
                            output.created_at::text AS created_at,
                            job.command_type
                        FROM job_outputs output
                        JOIN jobs job ON job.id = output.job_id
                        JOIN visible_clients client ON client.id = output.client_id
                        WHERE job.command_type IN (
                            'process_start',
                            'process_stop',
                            'process_restart',
                            'process_status',
                            'process_logs'
                        )
                          AND (
                            $1::text IS NULL
                            OR (
                                output.created_at,
                                output.job_id,
                                output.client_id,
                                output.seq
                            ) < (
                                $1::timestamptz,
                                $2::uuid,
                                $3::text,
                                $4::integer
                            )
                          )
                        ORDER BY
                            output.created_at DESC,
                            output.job_id DESC,
                            output.client_id DESC,
                            output.seq DESC
                        LIMIT $5
                        "#,
                    )
                    .bind(after_created_at.as_deref())
                    .bind(after_job_id)
                    .bind(after_client_id.as_deref())
                    .bind(after_seq)
                    .bind(fetch_limit as i64)
                    .fetch_all(&mut *tx)
                    .await?;
                    if rows.is_empty() {
                        history_exhausted = true;
                        break;
                    }
                    let row_count = rows.len();
                    let rows_to_process = row_count.min(remaining);
                    let mut outputs = Vec::with_capacity(rows_to_process);
                    for row in rows.into_iter().take(rows_to_process) {
                        let job_id: Uuid = row.try_get("job_id")?;
                        let client_id: String = row.try_get("client_id")?;
                        let seq: i32 = row.try_get("seq")?;
                        let created_at: String = row.try_get("created_at")?;
                        after_created_at = Some(created_at.clone());
                        after_job_id = Some(job_id);
                        after_client_id = Some(client_id.clone());
                        after_seq = Some(seq);
                        let command_type: String = row.try_get("command_type")?;
                        if is_process_supervisor_command(&command_type) {
                            outputs.push(SupervisorInventoryOutput {
                                job_id,
                                client_id,
                                stream: row.try_get("stream")?,
                                data: row.try_get("data")?,
                                created_at,
                                command_type,
                            });
                        }
                    }
                    scanned += rows_to_process;
                    if append_process_supervisor_inventory(
                        outputs,
                        &mut seen,
                        &mut inventory,
                        wanted,
                    ) {
                        break;
                    }
                    if row_count < fetch_limit {
                        history_exhausted = true;
                        break;
                    }
                }
                ensure_process_supervisor_inventory_complete(
                    inventory.len(),
                    wanted,
                    history_exhausted,
                )?;
                tx.commit().await?;
                Ok(inventory)
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn record_job_output_chunk_checked_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        output: &CommandOutput,
        received_at: Option<String>,
        config: JobOutputPersistConfig<'_>,
    ) -> Result<JobOutputWriteResult> {
        let mut results = self
            .record_job_outputs_starting_at(
                job_id,
                client_id,
                seq,
                std::slice::from_ref(output),
                received_at,
                config,
                false,
                None,
            )
            .await?;
        Ok(results.pop().unwrap_or(JobOutputWriteResult::Inserted))
    }

    pub(crate) async fn record_active_job_output_chunk_checked_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        output: &CommandOutput,
        received_at: Option<String>,
        config: JobOutputPersistConfig<'_>,
    ) -> Result<JobOutputWriteResult> {
        let mut results = self
            .record_job_outputs_starting_at(
                job_id,
                client_id,
                seq,
                std::slice::from_ref(output),
                received_at,
                config,
                true,
                None,
            )
            .await?;
        Ok(results.pop().unwrap_or(JobOutputWriteResult::Inserted))
    }

    pub(crate) async fn record_active_job_output_chunk_and_finalize_if_ready_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        output: &CommandOutput,
        received_at: Option<String>,
        config: JobOutputPersistConfig<'_>,
    ) -> Result<ActiveJobOutputChunkRecordResult> {
        if output.done {
            anyhow::bail!("non-final job output recorder received done output");
        }
        let mut persisted = materialize_job_outputs(
            job_id,
            client_id,
            seq,
            std::slice::from_ref(output),
            received_at,
            config,
        )
        .await?;
        let created_object_keys = persisted
            .iter()
            .filter_map(|output| output.created_artifact_object_key.clone())
            .collect::<Vec<_>>();
        let Some(stored_output) = persisted.pop() else {
            anyhow::bail!("job output materialization produced no rows");
        };
        let mut orphaned_object_keys = Vec::new();
        let operation: Result<ActiveJobOutputChunkRecordResult> = async {
            match self {
                Self::Postgres(pool) => {
                    let mut tx = pool.begin().await?;
                    let target_active =
                        lock_job_output_target_active_state_in_tx(&mut tx, job_id, client_id, None)
                            .await?;
                    let (lock_a, lock_b) = append_lock_keys(job_id, client_id);
                    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
                        .bind(lock_a)
                        .bind(lock_b)
                        .execute(&mut *tx)
                        .await?;
                    let existing = sqlx::query(
                        r#"
                    SELECT stream, data, storage, object_key, data_sha256_hex,
                           data_size_bytes, exit_code, done
                    FROM job_outputs
                    WHERE job_id = $1 AND client_id = $2 AND seq = $3
                    "#,
                    )
                    .bind(job_id)
                    .bind(client_id)
                    .bind(seq)
                    .fetch_optional(&mut *tx)
                    .await?;
                    let write_result = match existing {
                        Some(row) if job_output_row_matches_stored(&row, &stored_output) => {
                            JobOutputWriteResult::DuplicateIdentical
                        }
                        Some(_) => {
                            if let Some(object_key) =
                                stored_output.created_artifact_object_key.clone()
                            {
                                orphaned_object_keys.push(object_key);
                            }
                            insert_job_output_conflict_audit(&mut tx, &stored_output).await?;
                            JobOutputWriteResult::DuplicateConflict
                        }
                        None => {
                            if !target_active {
                                anyhow::bail!("job_target_not_active");
                            }
                            sqlx::query(
                                r#"
                            INSERT INTO job_outputs (
                                job_id, client_id, seq, stream, data, storage, object_key,
                                data_sha256_hex, data_size_bytes, exit_code, done, received_at
                            )
                            VALUES (
                                $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::timestamptz
                            )
                            "#,
                            )
                            .bind(job_id)
                            .bind(client_id)
                            .bind(seq)
                            .bind(&stored_output.stream)
                            .bind(&stored_output.data)
                            .bind(&stored_output.storage)
                            .bind(&stored_output.artifact_object_key)
                            .bind(&stored_output.artifact_sha256_hex)
                            .bind(stored_output.artifact_size_bytes)
                            .bind(stored_output.exit_code)
                            .bind(stored_output.done)
                            .bind(&stored_output.received_at)
                            .execute(&mut *tx)
                            .await?;
                            JobOutputWriteResult::Inserted
                        }
                    };
                    let contiguous_final =
                        if write_result == JobOutputWriteResult::DuplicateConflict {
                            None
                        } else {
                            contiguous_final_job_output_candidate_in_tx(&mut tx, job_id, client_id)
                                .await?
                        };
                    let terminal_reconciliation_ready = if let Some(candidate) =
                        contiguous_final.as_ref()
                    {
                        let received_at = candidate
                            .received_at
                            .clone()
                            .unwrap_or_else(|| Utc::now().to_rfc3339());
                        let outcome = crate::job_traffic_import::target_outcome_from_done_output(
                            job_id,
                            &candidate.output,
                            received_at,
                        );
                        terminalize_job_target_from_output_in_tx(
                            &mut tx,
                            job_id,
                            client_id,
                            candidate.seq,
                            &outcome,
                            JobOutputWriteResult::DuplicateIdentical,
                        )
                        .await?
                    } else {
                        false
                    };
                    tx.commit().await?;
                    Ok(ActiveJobOutputChunkRecordResult {
                        write_result,
                        contiguous_final,
                        terminal_reconciliation_ready,
                    })
                }
            }
        }
        .await;
        let result = match operation {
            Ok(result) => result,
            Err(error) => {
                if let Some(store) = config.object_store {
                    for object_key in created_object_keys {
                        store.delete_best_effort(&object_key).await;
                    }
                }
                return Err(error);
            }
        };
        if let Some(store) = config.object_store {
            for object_key in orphaned_object_keys {
                store.delete_best_effort(&object_key).await;
            }
        }
        Ok(result)
    }

    pub(crate) async fn record_active_final_job_output_and_target_result_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        output: &CommandOutput,
        received_at: Option<String>,
        config: JobOutputPersistConfig<'_>,
        outcome: &TargetDispatchOutcome,
    ) -> Result<FinalJobOutputRecordResult> {
        self.record_active_final_job_output_and_target_result_for_dispatch_attempt_with_config(
            job_id,
            client_id,
            seq,
            output,
            received_at,
            config,
            outcome,
            None,
        )
        .await
    }

    pub(crate) async fn record_claimed_final_job_output_and_target_result_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        output: &CommandOutput,
        received_at: Option<String>,
        config: JobOutputPersistConfig<'_>,
        outcome: &TargetDispatchOutcome,
        dispatch_attempt: i32,
    ) -> Result<FinalJobOutputRecordResult> {
        self.record_active_final_job_output_and_target_result_for_dispatch_attempt_with_config(
            job_id,
            client_id,
            seq,
            output,
            received_at,
            config,
            outcome,
            Some(dispatch_attempt),
        )
        .await
    }

    async fn record_active_final_job_output_and_target_result_for_dispatch_attempt_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        output: &CommandOutput,
        received_at: Option<String>,
        config: JobOutputPersistConfig<'_>,
        outcome: &TargetDispatchOutcome,
        expected_dispatch_attempt: Option<i32>,
    ) -> Result<FinalJobOutputRecordResult> {
        if !output.done {
            anyhow::bail!("final job output recorder requires done output");
        }
        let mut persisted = materialize_job_outputs(
            job_id,
            client_id,
            seq,
            std::slice::from_ref(output),
            received_at.clone(),
            config,
        )
        .await?;
        let created_object_keys = persisted
            .iter()
            .filter_map(|output| output.created_artifact_object_key.clone())
            .collect::<Vec<_>>();
        let Some(stored_output) = persisted.pop() else {
            anyhow::bail!("final output materialization produced no rows");
        };
        let mut orphaned_object_keys = Vec::new();
        let operation: Result<FinalJobOutputRecordResult> = async {
            match self {
                Self::Postgres(pool) => {
                    let mut tx = pool.begin().await?;
                    let target_active = lock_job_output_target_active_state_in_tx(
                        &mut tx,
                        job_id,
                        client_id,
                        expected_dispatch_attempt,
                    )
                    .await?;
                    if expected_dispatch_attempt.is_some() && !target_active {
                        anyhow::bail!("job_dispatch_attempt_stale");
                    }
                    let (lock_a, lock_b) = append_lock_keys(job_id, client_id);
                    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
                        .bind(lock_a)
                        .bind(lock_b)
                        .execute(&mut *tx)
                        .await?;
                    let existing = sqlx::query(
                        r#"
                    SELECT
                        stream,
                        data,
                        storage,
                        object_key,
                        data_sha256_hex,
                        data_size_bytes,
                        exit_code,
                        done
                    FROM job_outputs
                    WHERE job_id = $1 AND client_id = $2 AND seq = $3
                    "#,
                    )
                    .bind(stored_output.job_id)
                    .bind(&stored_output.client_id)
                    .bind(stored_output.seq)
                    .fetch_optional(&mut *tx)
                    .await?;
                    let write_result = match existing {
                        Some(row) if job_output_row_matches_stored(&row, &stored_output) => {
                            JobOutputWriteResult::DuplicateIdentical
                        }
                        Some(_) => {
                            if let Some(object_key) =
                                stored_output.created_artifact_object_key.clone()
                            {
                                orphaned_object_keys.push(object_key);
                            }
                            insert_job_output_conflict_audit(&mut tx, &stored_output).await?;
                            JobOutputWriteResult::DuplicateConflict
                        }
                        None => {
                            if !target_active {
                                anyhow::bail!("job_target_not_active");
                            }
                            let inserted = sqlx::query(
                                r#"
                            INSERT INTO job_outputs (
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
                                received_at
                            )
                            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::timestamptz)
                            ON CONFLICT (job_id, client_id, seq)
                            DO NOTHING
                            "#,
                            )
                            .bind(stored_output.job_id)
                            .bind(&stored_output.client_id)
                            .bind(stored_output.seq)
                            .bind(&stored_output.stream)
                            .bind(&stored_output.data)
                            .bind(&stored_output.storage)
                            .bind(&stored_output.artifact_object_key)
                            .bind(&stored_output.artifact_sha256_hex)
                            .bind(stored_output.artifact_size_bytes)
                            .bind(stored_output.exit_code)
                            .bind(stored_output.done)
                            .bind(&stored_output.received_at)
                            .execute(&mut *tx)
                            .await?;
                            if inserted.rows_affected() == 0 {
                                anyhow::bail!(
                                    "job_output_sequence_conflict_after_preflight:{}:{}:{}",
                                    stored_output.job_id,
                                    stored_output.client_id,
                                    stored_output.seq
                                );
                            }
                            JobOutputWriteResult::Inserted
                        }
                    };
                    let terminal_reconciliation_ready =
                        if write_result == JobOutputWriteResult::DuplicateConflict {
                            false
                        } else {
                            terminalize_job_target_from_output_in_tx(
                                &mut tx,
                                job_id,
                                client_id,
                                seq,
                                outcome,
                                write_result,
                            )
                            .await?
                        };
                    tx.commit().await?;
                    Ok(FinalJobOutputRecordResult {
                        write_result,
                        terminal_reconciliation_ready,
                    })
                }
            }
        }
        .await;
        let result = match operation {
            Ok(result) => result,
            Err(error) => {
                if let Some(store) = config.object_store {
                    for object_key in created_object_keys {
                        store.delete_best_effort(&object_key).await;
                    }
                }
                return Err(error);
            }
        };
        if let Some(store) = config.object_store {
            for object_key in orphaned_object_keys {
                store.delete_best_effort(&object_key).await;
            }
        }
        Ok(result)
    }

    pub(crate) async fn classify_existing_job_output_chunk_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        output: &CommandOutput,
        config: JobOutputPersistConfig<'_>,
    ) -> Result<Option<JobOutputWriteResult>> {
        let expected = expected_stored_job_output(job_id, client_id, seq, output, config)?;
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        stream,
                        data,
                        storage,
                        object_key,
                        data_sha256_hex,
                        data_size_bytes,
                        exit_code,
                        done
                    FROM job_outputs
                    WHERE job_id = $1 AND client_id = $2 AND seq = $3
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(seq)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                if job_output_row_matches_stored(&row, &expected) {
                    Ok(Some(JobOutputWriteResult::DuplicateIdentical))
                } else {
                    Ok(Some(JobOutputWriteResult::DuplicateConflict))
                }
            }
        }
    }

    pub(crate) async fn contiguous_final_job_output_candidate(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<Option<PendingFinalJobOutput>> {
        match self {
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
                        received_at::text AS received_at,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE job_id = $1
                      AND client_id = $2
                      AND done = TRUE
                    ORDER BY seq
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                for row in rows {
                    let view = job_output_view_from_row(row)?;
                    if self
                        .job_output_sequence_contiguous(job_id, client_id, view.seq)
                        .await?
                    {
                        return Ok(Some(PendingFinalJobOutput {
                            seq: view.seq,
                            output: command_output_from_view(&view)?,
                            received_at: view.received_at,
                        }));
                    }
                }
                Ok(None)
            }
        }
    }

    async fn job_output_sequence_contiguous(
        &self,
        job_id: Uuid,
        client_id: &str,
        final_seq: i32,
    ) -> Result<bool> {
        match self {
            Self::Postgres(pool) => {
                if final_seq < 0 {
                    return Ok(false);
                }
                let count: i64 = sqlx::query_scalar(
                    r#"
                    SELECT COUNT(DISTINCT seq)
                    FROM job_outputs
                    WHERE job_id = $1
                      AND client_id = $2
                      AND seq >= 0
                      AND seq <= $3
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(final_seq)
                .fetch_one(pool)
                .await?;
                Ok(count == i64::from(final_seq) + 1)
            }
        }
    }

    pub(crate) async fn record_job_output_sequence_conflict_audit(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
    ) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, 'job.output_conflict_ignored', $2, NULL, $3)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(format!("client:{client_id}"))
                .bind(serde_json::json!({
                    "job_id": job_id,
                    "client_id": client_id,
                    "seq": seq,
                    "reason": "output sequence already persisted with different content",
                    "result": "ignored",
                    "origin_kind": "gateway_ingest",
                    "component": "gateway-command-output-ingest",
                }))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn record_job_outputs_checked_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        outputs: &[CommandOutput],
        config: JobOutputPersistConfig<'_>,
    ) -> Result<Vec<JobOutputWriteResult>> {
        self.record_job_outputs_starting_at(
            job_id, client_id, 0, outputs, None, config, false, None,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn record_active_job_outputs_checked_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        outputs: &[CommandOutput],
        config: JobOutputPersistConfig<'_>,
    ) -> Result<Vec<JobOutputWriteResult>> {
        self.record_job_outputs_starting_at(job_id, client_id, 0, outputs, None, config, true, None)
            .await
    }

    pub(crate) async fn record_claimed_job_outputs_checked_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        outputs: &[CommandOutput],
        config: JobOutputPersistConfig<'_>,
        dispatch_attempt: i32,
    ) -> Result<Vec<JobOutputWriteResult>> {
        self.record_job_outputs_starting_at(
            job_id,
            client_id,
            0,
            outputs,
            None,
            config,
            true,
            Some(dispatch_attempt),
        )
        .await
    }

    async fn record_job_outputs_starting_at(
        &self,
        job_id: Uuid,
        client_id: &str,
        start_seq: i32,
        outputs: &[CommandOutput],
        received_at: Option<String>,
        config: JobOutputPersistConfig<'_>,
        require_active_target: bool,
        expected_dispatch_attempt: Option<i32>,
    ) -> Result<Vec<JobOutputWriteResult>> {
        if outputs.is_empty() {
            return Ok(Vec::new());
        }
        let persisted =
            materialize_job_outputs(job_id, client_id, start_seq, outputs, received_at, config)
                .await?;
        let object_keys = persisted
            .iter()
            .filter_map(|output| output.created_artifact_object_key.clone())
            .collect::<Vec<_>>();
        let mut orphaned_object_keys = Vec::new();
        let result: Result<Vec<JobOutputWriteResult>> = async {
            match self {
                Self::Postgres(pool) => {
                    let mut tx = pool.begin().await?;
                    let target_active = if require_active_target {
                        Some(
                            lock_job_output_target_active_state_in_tx(
                                &mut tx,
                                job_id,
                                client_id,
                                expected_dispatch_attempt,
                            )
                            .await?,
                        )
                    } else {
                        None
                    };
                    if expected_dispatch_attempt.is_some() && target_active == Some(false) {
                        anyhow::bail!("job_dispatch_attempt_stale");
                    }
                    let (lock_a, lock_b) = append_lock_keys(job_id, client_id);
                    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
                        .bind(lock_a)
                        .bind(lock_b)
                        .execute(&mut *tx)
                        .await?;
                    let mut planned_results = Vec::with_capacity(persisted.len());
                    let mut has_conflict = false;
                    let mut conflict_outputs = Vec::new();
                    for output in &persisted {
                        let existing = sqlx::query(
                            r#"
                        SELECT
                            stream,
                            data,
                            storage,
                            object_key,
                            data_sha256_hex,
                            data_size_bytes,
                            exit_code,
                            done
                        FROM job_outputs
                        WHERE job_id = $1 AND client_id = $2 AND seq = $3
                        "#,
                        )
                        .bind(output.job_id)
                        .bind(&output.client_id)
                        .bind(output.seq)
                        .fetch_optional(&mut *tx)
                        .await?;
                        match existing {
                            Some(row) if job_output_row_matches_stored(&row, output) => {
                                planned_results.push(JobOutputWriteResult::DuplicateIdentical);
                            }
                            Some(_) => {
                                planned_results.push(JobOutputWriteResult::DuplicateConflict);
                                conflict_outputs.push(output.clone());
                                has_conflict = true;
                                if let Some(object_key) = output.created_artifact_object_key.clone()
                                {
                                    orphaned_object_keys.push(object_key);
                                }
                            }
                            None => {
                                planned_results.push(JobOutputWriteResult::Inserted);
                            }
                        }
                    }
                    if !has_conflict
                        && target_active == Some(false)
                        && planned_results.contains(&JobOutputWriteResult::Inserted)
                    {
                        anyhow::bail!("job_target_not_active");
                    }
                    if has_conflict {
                        for (output, result) in persisted.iter().zip(planned_results.iter_mut()) {
                            if *result == JobOutputWriteResult::Inserted {
                                *result = JobOutputWriteResult::DuplicateConflict;
                            }
                            if let Some(object_key) = output.created_artifact_object_key.clone() {
                                orphaned_object_keys.push(object_key);
                            }
                        }
                        for output in &conflict_outputs {
                            insert_job_output_conflict_audit(&mut tx, output).await?;
                        }
                    } else {
                        for (output, result) in persisted.iter().zip(planned_results.iter()) {
                            if *result != JobOutputWriteResult::Inserted {
                                continue;
                            }
                            let inserted = sqlx::query(
                                r#"
                            INSERT INTO job_outputs (
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
                                received_at
                            )
                            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::timestamptz)
                            ON CONFLICT (job_id, client_id, seq)
                            DO NOTHING
                            "#,
                            )
                            .bind(output.job_id)
                            .bind(&output.client_id)
                            .bind(output.seq)
                            .bind(&output.stream)
                            .bind(&output.data)
                            .bind(&output.storage)
                            .bind(&output.artifact_object_key)
                            .bind(&output.artifact_sha256_hex)
                            .bind(output.artifact_size_bytes)
                            .bind(output.exit_code)
                            .bind(output.done)
                            .bind(&output.received_at)
                            .execute(&mut *tx)
                            .await?;
                            if inserted.rows_affected() == 0 {
                                anyhow::bail!(
                                    "job_output_sequence_conflict_after_preflight:{}:{}:{}",
                                    output.job_id,
                                    output.client_id,
                                    output.seq
                                );
                            }
                        }
                        if require_active_target
                            && planned_results.contains(&JobOutputWriteResult::Inserted)
                        {
                            enqueue_network_traffic_import_finalization_if_ready_in_tx(
                                &mut tx, job_id, client_id,
                            )
                            .await?;
                        }
                    }
                    tx.commit().await?;
                    Ok(planned_results)
                }
            }
        }
        .await;
        let write_results = match result {
            Ok(write_results) => write_results,
            Err(error) => {
                if let Some(store) = config.object_store {
                    for object_key in object_keys {
                        store.delete_best_effort(&object_key).await;
                    }
                }
                return Err(error);
            }
        };
        if let Some(store) = config.object_store {
            for object_key in orphaned_object_keys {
                store.delete_best_effort(&object_key).await;
            }
        }
        Ok(write_results)
    }
}

#[derive(Clone, Debug)]
struct StoredJobOutput {
    job_id: Uuid,
    client_id: String,
    seq: i32,
    stream: String,
    data: Vec<u8>,
    storage: String,
    artifact_object_key: Option<String>,
    created_artifact_object_key: Option<String>,
    artifact_sha256_hex: Option<String>,
    artifact_size_bytes: Option<i64>,
    exit_code: Option<i32>,
    done: bool,
    received_at: String,
}

fn job_output_view_server_artifact(output: &JobOutputView) -> Option<NewServerArtifact> {
    Some(NewServerArtifact {
        domain: "job_output".to_string(),
        object_key: output.artifact_object_key.clone()?,
        sha256_hex: output.artifact_sha256_hex.clone()?,
        size_bytes: output.artifact_size_bytes?,
        job_id: Some(output.job_id),
        client_id: Some(output.client_id.clone()),
        stream: Some(output.stream.clone()),
        seq: Some(output.seq),
        backup_request_id: None,
        backup_artifact_id: None,
        release_id: None,
        metadata: serde_json::json!({}),
    })
}

fn job_output_view_from_row(row: PgRow) -> std::result::Result<JobOutputView, sqlx::Error> {
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
        received_at: row.try_get("received_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn job_output_list_item_from_row(
    row: PgRow,
) -> std::result::Result<JobOutputListItemView, sqlx::Error> {
    let data: Option<Vec<u8>> = row.try_get("data")?;
    Ok(JobOutputListItemView {
        job_id: row.try_get("job_id")?,
        client_id: row.try_get("client_id")?,
        seq: row.try_get("seq")?,
        stream: row.try_get("stream")?,
        data_base64: data.map(|data| BASE64.encode(data)),
        storage: row.try_get("storage")?,
        artifact_object_key: row.try_get("object_key")?,
        artifact_sha256_hex: row.try_get("data_sha256_hex")?,
        artifact_size_bytes: row.try_get("data_size_bytes")?,
        exit_code: row.try_get("exit_code")?,
        done: row.try_get("done")?,
        received_at: row.try_get("received_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn command_output_from_view(view: &JobOutputView) -> Result<CommandOutput> {
    Ok(CommandOutput {
        job_id: view.job_id,
        stream: output_stream_from_name(&view.stream)?,
        data: BASE64.decode(&view.data_base64)?,
        exit_code: view.exit_code,
        done: view.done,
    })
}

fn output_stream_from_name(value: &str) -> Result<OutputStream> {
    match value {
        "stdout" => Ok(OutputStream::Stdout),
        "stderr" => Ok(OutputStream::Stderr),
        "pty" => Ok(OutputStream::Pty),
        "status" => Ok(OutputStream::Status),
        _ => anyhow::bail!("unknown job output stream: {value}"),
    }
}

async fn enqueue_network_traffic_import_finalization_if_ready_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO network_traffic_import_finalizations (
            job_id, client_id, final_seq
        )
        SELECT $1, $2, final_output.seq
        FROM jobs job
        JOIN job_targets target
          ON target.job_id = job.id
         AND target.client_id = $2
        JOIN LATERAL (
            SELECT output.seq
            FROM job_outputs output
            WHERE output.job_id = job.id
              AND output.client_id = $2
              AND output.done = TRUE
              AND output.seq >= 0
              AND (
                  SELECT COUNT(*)
                  FROM job_outputs chunk
                  WHERE chunk.job_id = output.job_id
                    AND chunk.client_id = output.client_id
                    AND chunk.seq BETWEEN 0 AND output.seq
              ) = output.seq::bigint + 1
            ORDER BY output.seq
            LIMIT 1
        ) final_output ON TRUE
        WHERE job.id = $1
          AND job.command_type = 'network_traffic_import_vnstat'
          AND target.completed_at IS NULL
          AND target.status IN ('dispatching', 'running')
        ON CONFLICT (job_id, client_id) DO NOTHING
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn job_output_sequence_contiguous_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
    client_id: &str,
    final_seq: i32,
) -> Result<bool> {
    if final_seq < 0 {
        return Ok(false);
    }
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT seq)
        FROM job_outputs
        WHERE job_id = $1
          AND client_id = $2
          AND seq >= 0
          AND seq <= $3
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(final_seq)
    .fetch_one(&mut **tx)
    .await?;
    Ok(count == i64::from(final_seq) + 1)
}

async fn contiguous_final_job_output_candidate_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
    client_id: &str,
) -> Result<Option<PendingFinalJobOutput>> {
    let row = sqlx::query(
        r#"
        SELECT
            final_output.job_id,
            final_output.client_id,
            final_output.seq,
            final_output.stream,
            final_output.data,
            final_output.storage,
            final_output.object_key,
            final_output.data_sha256_hex,
            final_output.data_size_bytes,
            final_output.exit_code,
            final_output.done,
            final_output.received_at::text AS received_at,
            final_output.created_at::text AS created_at
        FROM job_outputs final_output
        WHERE final_output.job_id = $1
          AND final_output.client_id = $2
          AND final_output.done = TRUE
          AND final_output.seq >= 0
          AND (
              SELECT COUNT(DISTINCT chunk.seq)
              FROM job_outputs chunk
              WHERE chunk.job_id = final_output.job_id
                AND chunk.client_id = final_output.client_id
                AND chunk.seq BETWEEN 0 AND final_output.seq
          ) = final_output.seq::bigint + 1
        ORDER BY final_output.seq
        LIMIT 1
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let view = job_output_view_from_row(row)?;
    Ok(Some(PendingFinalJobOutput {
        seq: view.seq,
        output: command_output_from_view(&view)?,
        received_at: view.received_at,
    }))
}

async fn terminalize_job_target_from_output_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
    client_id: &str,
    output_seq: i32,
    outcome: &TargetDispatchOutcome,
    write_result: JobOutputWriteResult,
) -> Result<bool> {
    if !job_output_sequence_contiguous_in_tx(tx, job_id, client_id, output_seq).await? {
        return Ok(false);
    }
    let updated = sqlx::query(
        r#"
        UPDATE job_targets
        SET status = $3,
            message = $4,
            exit_code = $5,
            started_at = COALESCE(started_at, now()),
            completed_at = now(),
            result_received_at = COALESCE($6::timestamptz, now()),
            dispatch_lease_until = NULL,
            last_dispatch_error = CASE
                WHEN $3 IN ('failed', 'control_timeout', 'agent_lost') THEN $4
                ELSE NULL
            END
        WHERE job_id = $1
          AND client_id = $2
          AND completed_at IS NULL
          AND status IN ('queued', 'dispatching', 'running')
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(&outcome.status)
    .bind(&outcome.message)
    .bind(outcome.exit_code)
    .bind(outcome.received_at.as_deref())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 0 && write_result != JobOutputWriteResult::DuplicateIdentical {
        anyhow::bail!("job_target_not_active");
    }
    let target_terminalized = updated.rows_affected() > 0;
    let terminal_reconciliation_ready = if target_terminalized {
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_id, action, target, command_hash, metadata
            )
            VALUES (
                $1, NULL, 'job.target_result', $2,
                (SELECT payload_hash FROM jobs WHERE id = $3),
                $4
            )
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(format!("client:{client_id}"))
        .bind(job_id)
        .bind(serde_json::json!({
            "job_id": job_id,
            "status": outcome.status,
            "result": outcome.status,
            "exit_code": outcome.exit_code,
            "accepted": outcome.accepted,
            "message": outcome.message,
            "received_at": outcome.received_at,
            "output_seq": output_seq,
            "origin_kind": "gateway_ingest",
            "component": "gateway-command-output-ingest",
        }))
        .execute(&mut **tx)
        .await?;
        enqueue_target_terminal_event_in_tx(tx, job_id, client_id, outcome).await?;
        insert_agent_update_lifecycle_for_stored_job_in_tx(tx, job_id, client_id, outcome).await?;
        true
    } else {
        sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM audit_logs
                WHERE action = 'job.target_result'
                  AND target = $1
                  AND metadata->>'job_id' = $2
                  AND metadata->>'output_seq' = $3
                  AND metadata->>'component' = 'gateway-command-output-ingest'
            )
            "#,
        )
        .bind(format!("client:{client_id}"))
        .bind(job_id.to_string())
        .bind(output_seq.to_string())
        .fetch_one(&mut **tx)
        .await?
    };
    finish_jobs_in_tx_and_reconcile_event_sources(tx, &[job_id]).await?;
    Ok(terminal_reconciliation_ready)
}

fn job_output_row_matches_stored(row: &sqlx::postgres::PgRow, output: &StoredJobOutput) -> bool {
    let Ok(stream) = row.try_get::<String, _>("stream") else {
        return false;
    };
    let Ok(data) = row.try_get::<Vec<u8>, _>("data") else {
        return false;
    };
    let Ok(storage) = row.try_get::<String, _>("storage") else {
        return false;
    };
    let Ok(object_key) = row.try_get::<Option<String>, _>("object_key") else {
        return false;
    };
    let Ok(data_sha256_hex) = row.try_get::<Option<String>, _>("data_sha256_hex") else {
        return false;
    };
    let Ok(data_size_bytes) = row.try_get::<Option<i64>, _>("data_size_bytes") else {
        return false;
    };
    let Ok(exit_code) = row.try_get::<Option<i32>, _>("exit_code") else {
        return false;
    };
    let Ok(done) = row.try_get::<bool, _>("done") else {
        return false;
    };
    stream == output.stream.as_str()
        && data.as_slice() == output.data.as_slice()
        && storage == output.storage.as_str()
        && object_key.as_ref() == output.artifact_object_key.as_ref()
        && data_sha256_hex.as_ref() == output.artifact_sha256_hex.as_ref()
        && data_size_bytes == output.artifact_size_bytes
        && exit_code == output.exit_code
        && done == output.done
}

async fn lock_job_output_target_active_state_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
    client_id: &str,
    expected_dispatch_attempt: Option<i32>,
) -> Result<bool> {
    let row = sqlx::query(
        r#"
        SELECT status, completed_at::text AS completed_at, dispatch_attempts
        FROM job_targets
        WHERE job_id = $1 AND client_id = $2
        FOR UPDATE
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        anyhow::bail!("job_target_not_found");
    };
    let status: String = row.try_get("status")?;
    let completed_at: Option<String> = row.try_get("completed_at")?;
    let dispatch_attempts: i32 = row.try_get("dispatch_attempts")?;
    Ok(completed_at.is_none()
        && target_status_is_active(&status)
        && expected_dispatch_attempt.is_none_or(|expected| expected == dispatch_attempts))
}

fn expected_stored_job_output(
    job_id: Uuid,
    client_id: &str,
    seq: i32,
    output: &CommandOutput,
    config: JobOutputPersistConfig<'_>,
) -> Result<StoredJobOutput> {
    let stream = output_stream_name(output.stream).to_string();
    if output.stream == OutputStream::Status && output.data.len() > STATUS_OUTPUT_MAX_BYTES {
        anyhow::bail!(
            "status output exceeds max bytes: {} > {}",
            output.data.len(),
            STATUS_OUTPUT_MAX_BYTES
        );
    }
    if should_externalize_output(output, &config) {
        let sha256_hex = payload_hash(&output.data);
        let object_key = job_output_object_key(job_id, client_id, seq, &stream, &sha256_hex);
        Ok(StoredJobOutput {
            job_id,
            client_id: client_id.to_string(),
            seq,
            stream,
            data: output
                .data
                .iter()
                .copied()
                .take(INLINE_OUTPUT_PREVIEW_BYTES)
                .collect(),
            storage: "object_store".to_string(),
            artifact_object_key: Some(object_key),
            created_artifact_object_key: None,
            artifact_sha256_hex: Some(sha256_hex),
            artifact_size_bytes: Some(output.data.len() as i64),
            exit_code: output.exit_code,
            done: output.done,
            received_at: String::new(),
        })
    } else {
        Ok(StoredJobOutput {
            job_id,
            client_id: client_id.to_string(),
            seq,
            stream,
            data: output.data.clone(),
            storage: "inline".to_string(),
            artifact_object_key: None,
            created_artifact_object_key: None,
            artifact_sha256_hex: Some(payload_hash(&output.data)),
            artifact_size_bytes: Some(output.data.len() as i64),
            exit_code: output.exit_code,
            done: output.done,
            received_at: String::new(),
        })
    }
}

async fn insert_job_output_conflict_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    output: &StoredJobOutput,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, 'job.output_conflict_ignored', $2, NULL, $3)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(format!("client:{}", output.client_id))
    .bind(serde_json::json!({
        "job_id": output.job_id,
        "client_id": output.client_id,
        "seq": output.seq,
        "reason": "output sequence already persisted with different content",
        "result": "ignored",
        "origin_kind": "gateway_ingest",
        "component": "gateway-command-output-ingest",
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn materialize_job_outputs(
    job_id: Uuid,
    client_id: &str,
    start_seq: i32,
    outputs: &[CommandOutput],
    received_at: Option<String>,
    config: JobOutputPersistConfig<'_>,
) -> Result<Vec<StoredJobOutput>> {
    let received_at = received_at.unwrap_or_else(|| Utc::now().to_rfc3339());
    let mut persisted = Vec::with_capacity(outputs.len());
    for (index, output) in outputs.iter().enumerate() {
        let seq = start_seq
            .checked_add(i32::try_from(index)?)
            .ok_or_else(|| anyhow::anyhow!("job output sequence overflow"))?;
        let should_externalize = should_externalize_output(output, &config);
        let stream = output_stream_name(output.stream).to_string();
        if output.stream == OutputStream::Status && output.data.len() > STATUS_OUTPUT_MAX_BYTES {
            anyhow::bail!(
                "status output exceeds max bytes: {} > {}",
                output.data.len(),
                STATUS_OUTPUT_MAX_BYTES
            );
        }
        if should_externalize {
            let sha256_hex = payload_hash(&output.data);
            let object_key = job_output_object_key(job_id, client_id, seq, &stream, &sha256_hex);
            let store = config
                .object_store
                .expect("object store exists when should_externalize_output is true");
            let created_artifact_object_key =
                put_job_output_object(store, &object_key, &output.data)
                    .await?
                    .then_some(object_key.clone());
            persisted.push(StoredJobOutput {
                job_id,
                client_id: client_id.to_string(),
                seq,
                stream,
                data: output
                    .data
                    .iter()
                    .copied()
                    .take(INLINE_OUTPUT_PREVIEW_BYTES)
                    .collect(),
                storage: "object_store".to_string(),
                artifact_object_key: Some(object_key),
                created_artifact_object_key,
                artifact_sha256_hex: Some(sha256_hex),
                artifact_size_bytes: Some(output.data.len() as i64),
                exit_code: output.exit_code,
                done: output.done,
                received_at: received_at.clone(),
            });
        } else {
            persisted.push(StoredJobOutput {
                job_id,
                client_id: client_id.to_string(),
                seq,
                stream,
                data: output.data.clone(),
                storage: "inline".to_string(),
                artifact_object_key: None,
                created_artifact_object_key: None,
                artifact_sha256_hex: Some(payload_hash(&output.data)),
                artifact_size_bytes: Some(output.data.len() as i64),
                exit_code: output.exit_code,
                done: output.done,
                received_at: received_at.clone(),
            });
        }
    }
    Ok(persisted)
}

async fn put_job_output_object(
    store: &BackupObjectStore,
    object_key: &str,
    data: &[u8],
) -> Result<bool> {
    match store.put_new(object_key, data).await {
        Ok(()) => Ok(true),
        Err(error) => match store.get_with_limit(object_key, data.len()).await {
            Ok(existing) if existing == data => Ok(false),
            _ => Err(error),
        },
    }
}

fn should_externalize_output(output: &CommandOutput, config: &JobOutputPersistConfig<'_>) -> bool {
    config.object_store.is_some()
        && config.artifact_min_bytes > 0
        && output.stream != OutputStream::Status
        && output.data.len() >= config.artifact_min_bytes
}

fn job_output_object_key(
    job_id: Uuid,
    client_id: &str,
    seq: i32,
    stream: &str,
    sha256_hex: &str,
) -> String {
    let client_hex = hex::encode(client_id.as_bytes());
    format!("{JOB_OUTPUT_ARTIFACT_PREFIX}/{job_id}/{client_hex}/{seq}-{stream}-{sha256_hex}.bin")
}

pub(crate) fn append_lock_keys(job_id: Uuid, client_id: &str) -> (i32, i32) {
    let mut left = 0x811c_9dc5_u32;
    let mut right = 0x0100_0193_u32;
    for byte in job_id.as_bytes().iter().chain(client_id.as_bytes()) {
        left ^= u32::from(*byte);
        left = left.wrapping_mul(0x0100_0193);
        right = right.rotate_left(5) ^ u32::from(*byte);
        right = right.wrapping_mul(0x85eb_ca6b);
    }
    (left as i32, right as i32)
}

#[derive(Clone, Debug)]
struct SupervisorInventoryOutput {
    job_id: Uuid,
    client_id: String,
    stream: String,
    data: Vec<u8>,
    created_at: String,
    command_type: String,
}

#[cfg(test)]
fn build_process_supervisor_inventory(
    outputs: Vec<SupervisorInventoryOutput>,
    limit: i64,
) -> Vec<ProcessSupervisorInventoryView> {
    let mut seen = BTreeSet::<(String, String)>::new();
    let mut inventory = Vec::new();
    let limit = limit.clamp(1, 200) as usize;
    append_process_supervisor_inventory(outputs, &mut seen, &mut inventory, limit);
    inventory
}

fn ensure_process_supervisor_inventory_complete(
    inventory_len: usize,
    wanted: usize,
    history_exhausted: bool,
) -> Result<()> {
    if inventory_len < wanted && !history_exhausted {
        anyhow::bail!(PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR);
    }
    Ok(())
}

fn append_process_supervisor_inventory(
    outputs: impl IntoIterator<Item = SupervisorInventoryOutput>,
    seen: &mut BTreeSet<(String, String)>,
    inventory: &mut Vec<ProcessSupervisorInventoryView>,
    limit: usize,
) -> bool {
    for output in outputs {
        for item in parse_process_supervisor_inventory_output(&output) {
            let key = (item.client_id.clone(), item.name.clone());
            if seen.insert(key) {
                inventory.push(item);
                if inventory.len() >= limit {
                    return true;
                }
            }
        }
    }
    false
}

fn parse_process_supervisor_inventory_output(
    output: &SupervisorInventoryOutput,
) -> Vec<ProcessSupervisorInventoryView> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.data) else {
        return Vec::new();
    };
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("process_status") if output.stream == "stdout" => value
            .get("processes")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|process| process_inventory_item(output, process))
            .collect(),
        Some("process_start" | "process_stop" | "process_restart" | "process_logs")
            if output.stream == "status" =>
        {
            process_inventory_item(output, &value).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

fn process_inventory_item(
    output: &SupervisorInventoryOutput,
    value: &serde_json::Value,
) -> Option<ProcessSupervisorInventoryView> {
    let name = value.get("name")?.as_str()?.to_string();
    Some(ProcessSupervisorInventoryView {
        client_id: output.client_id.clone(),
        name,
        status: value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        pid: value.get("pid").and_then(serde_json::Value::as_i64),
        process_exit_code: value
            .get("exit_code")
            .and_then(serde_json::Value::as_i64)
            .and_then(|code| i32::try_from(code).ok()),
        source_job_id: output.job_id,
        source_command_type: output.command_type.clone(),
        stdout_log: json_string(value, "stdout_log"),
        stderr_log: json_string(value, "stderr_log"),
        started_unix: value
            .get("started_unix")
            .and_then(serde_json::Value::as_u64),
        restart_attempts: value
            .get("restart_attempts")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u16::try_from(value).ok()),
        last_exit_code: value
            .get("last_exit_code")
            .and_then(serde_json::Value::as_i64)
            .and_then(|code| i32::try_from(code).ok()),
        last_exit_unix: value
            .get("last_exit_unix")
            .and_then(serde_json::Value::as_u64),
        last_restart_unix: value
            .get("last_restart_unix")
            .and_then(serde_json::Value::as_u64),
        limit_effectiveness_status: value
            .get("limit_effectiveness")
            .and_then(|value| value.get("overall"))
            .and_then(|value| value.get("status"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        cgroup_status: value
            .get("cgroup_status")
            .and_then(|value| value.get("status"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        cgroup_process_count: value
            .get("cgroup_status")
            .and_then(|value| value.get("process_count"))
            .and_then(serde_json::Value::as_u64),
        cgroup_cpu_weight: value
            .get("cgroup_status")
            .and_then(|value| value.get("cpu_weight"))
            .and_then(serde_json::Value::as_u64),
        cgroup_memory_current_bytes: value
            .get("cgroup_status")
            .and_then(|value| value.get("memory_current_bytes"))
            .and_then(serde_json::Value::as_u64),
        cgroup_pids_current: value
            .get("cgroup_status")
            .and_then(|value| value.get("pids_current"))
            .and_then(serde_json::Value::as_u64),
        observed_at: output.created_at.clone(),
    })
}

fn json_string(value: &serde_json::Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

fn is_process_supervisor_command(command_type: &str) -> bool {
    matches!(
        command_type,
        "process_start" | "process_stop" | "process_restart" | "process_status" | "process_logs"
    )
}

#[cfg(test)]
#[path = "tests_repository_job_outputs.rs"]
mod tests;
