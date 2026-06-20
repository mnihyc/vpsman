use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use sqlx::{postgres::PgRow, Row};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
use vpsman_common::{payload_hash, CommandOutput, OutputStream};
use vpsman_server_core::{
    aggregate_job_status_from_statuses, target_status_is_active, INLINE_OUTPUT_PREVIEW_BYTES,
    STATUS_OUTPUT_MAX_BYTES,
};

use crate::model::{
    AuditLogView, JobOutputListItemView, JobOutputView, NewServerArtifact,
    ProcessSupervisorInventoryView,
};
use crate::object_store::BackupObjectStore;
use crate::repository::{MemoryState, Repository};
use crate::repository_jobs::finish_job_in_tx_if_all_targets_terminal;
use crate::{output_stream_name, unix_now, TargetDispatchOutcome};

const JOB_OUTPUT_ARTIFACT_PREFIX: &str = "job-outputs";

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
    pub(crate) target_terminalized: bool,
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

impl Repository {
    pub(crate) async fn list_job_outputs(&self, job_id: Uuid) -> Result<Vec<JobOutputView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .job_outputs
                .read()
                .await
                .iter()
                .filter(|output| output.job_id == job_id)
                .cloned()
                .collect()),
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

    pub(crate) async fn list_job_outputs_page(
        &self,
        job_id: Uuid,
        filter: JobOutputListFilter,
    ) -> Result<Vec<JobOutputListItemView>> {
        let limit = filter.limit.clamp(1, 1001);
        match self {
            Self::Memory(memory) => {
                let mut outputs = memory
                    .job_outputs
                    .read()
                    .await
                    .iter()
                    .filter(|output| output.job_id == job_id)
                    .filter(|output| {
                        filter
                            .client_id
                            .as_ref()
                            .is_none_or(|client_id| &output.client_id == client_id)
                    })
                    .filter(|output| {
                        filter
                            .stream
                            .as_ref()
                            .is_none_or(|stream| &output.stream == stream)
                    })
                    .filter(|output| filter.seq_after.is_none_or(|seq| output.seq > seq))
                    .filter(|output| {
                        filter.cursor.as_ref().is_none_or(|cursor| {
                            output.client_id.as_str() > cursor.client_id.as_str()
                                || (output.client_id == cursor.client_id && output.seq > cursor.seq)
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                outputs.sort_by(|left, right| {
                    left.client_id
                        .cmp(&right.client_id)
                        .then_with(|| left.seq.cmp(&right.seq))
                });
                outputs.truncate(limit as usize);
                Ok(outputs
                    .into_iter()
                    .map(|output| output.into_list_item(filter.include_data))
                    .collect())
            }
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
            Self::Memory(memory) => Ok(memory
                .job_outputs
                .read()
                .await
                .iter()
                .find(|output| {
                    output.job_id == job_id && output.client_id == client_id && output.seq == seq
                })
                .cloned()),
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
            Self::Memory(memory) => Ok(memory
                .job_outputs
                .read()
                .await
                .iter()
                .find(|output| {
                    output.job_id == job_id && output.client_id == client_id && output.seq == seq
                })
                .and_then(|output| {
                    Some(JobOutputArtifactRef {
                        object_key: output.artifact_object_key.clone()?,
                        sha256_hex: output.artifact_sha256_hex.clone()?,
                        size_bytes: output.artifact_size_bytes?,
                    })
                })),
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

    pub(crate) async fn list_process_supervisor_inventory(
        &self,
        limit: i64,
    ) -> Result<Vec<ProcessSupervisorInventoryView>> {
        let scan_limit = limit.saturating_mul(32).clamp(50, 5_000);
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
                        let command_type = command_types.get(&output.job_id)?;
                        if !is_process_supervisor_command(command_type) {
                            return None;
                        }
                        let data = BASE64.decode(&output.data_base64).ok()?;
                        Some(SupervisorInventoryOutput {
                            job_id: output.job_id,
                            client_id: output.client_id.clone(),
                            stream: output.stream.clone(),
                            data,
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
                        .then_with(|| right.stream.cmp(&left.stream))
                });
                Ok(build_process_supervisor_inventory(outputs, limit))
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        output.job_id,
                        output.client_id,
                        output.stream,
                        output.data,
                        output.created_at::text AS created_at,
                        job.command_type
                    FROM job_outputs output
                    JOIN jobs job ON job.id = output.job_id
                    WHERE job.command_type IN (
                        'process_start',
                        'process_stop',
                        'process_restart',
                        'process_status',
                        'process_logs'
                    )
                    ORDER BY output.created_at DESC, output.job_id DESC, output.seq DESC
                    LIMIT $1
                    "#,
                )
                .bind(scan_limit)
                .fetch_all(pool)
                .await?;
                let outputs = rows
                    .into_iter()
                    .map(|row| {
                        Ok(SupervisorInventoryOutput {
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                            stream: row.try_get("stream")?,
                            data: row.try_get("data")?,
                            created_at: row.try_get("created_at")?,
                            command_type: row.try_get("command_type")?,
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
                Ok(build_process_supervisor_inventory(outputs, limit))
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn record_job_outputs(
        &self,
        job_id: Uuid,
        client_id: &str,
        outputs: &[CommandOutput],
    ) -> Result<()> {
        self.record_job_outputs_with_config(
            job_id,
            client_id,
            outputs,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn record_job_outputs_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        outputs: &[CommandOutput],
        config: JobOutputPersistConfig<'_>,
    ) -> Result<()> {
        if outputs.is_empty() {
            return Ok(());
        }
        let _ = self
            .record_job_outputs_starting_at(job_id, client_id, 0, outputs, None, config, false)
            .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn record_job_output_chunk_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        output: &CommandOutput,
        received_at: Option<String>,
        config: JobOutputPersistConfig<'_>,
    ) -> Result<()> {
        let _ = self
            .record_job_outputs_starting_at(
                job_id,
                client_id,
                seq,
                std::slice::from_ref(output),
                received_at,
                config,
                false,
            )
            .await?;
        Ok(())
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
            )
            .await?;
        Ok(results.pop().unwrap_or(JobOutputWriteResult::Inserted))
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
        let mut accepted_persisted = Vec::new();
        let mut conflict_audits = Vec::new();
        let mut terminal_status = None::<String>;
        let operation = match self {
            Self::Memory(memory) => {
                ensure_memory_job_output_target_active(memory, job_id, client_id).await?;
                let write_result = {
                    let mut stored = memory.job_outputs.write().await;
                    if let Some(existing) = stored.iter().find(|existing| {
                        existing.job_id == stored_output.job_id
                            && existing.client_id == stored_output.client_id
                            && existing.seq == stored_output.seq
                    }) {
                        if job_output_view_matches_stored(existing, &stored_output) {
                            accepted_persisted.push(stored_output.clone());
                            JobOutputWriteResult::DuplicateIdentical
                        } else {
                            if let Some(object_key) =
                                stored_output.created_artifact_object_key.clone()
                            {
                                orphaned_object_keys.push(object_key);
                            }
                            conflict_audits.push(job_output_conflict_audit(
                                stored_output.job_id,
                                &stored_output.client_id,
                                stored_output.seq,
                            ));
                            JobOutputWriteResult::DuplicateConflict
                        }
                    } else {
                        stored.push(stored_output.clone().into_view());
                        accepted_persisted.push(stored_output.clone());
                        JobOutputWriteResult::Inserted
                    }
                };
                let mut target_terminalized = false;
                if write_result != JobOutputWriteResult::DuplicateConflict {
                    let completed_at = unix_now().to_string();
                    let statuses = {
                        let mut targets = memory.job_targets.write().await;
                        let Some(target) = targets.iter_mut().find(|target| {
                            target.job_id == job_id
                                && target.client_id == client_id
                                && target.completed_at.is_none()
                                && target_status_is_active(&target.status)
                        }) else {
                            anyhow::bail!("job_target_not_active");
                        };
                        target.status = outcome.status.clone();
                        target.message = Some(outcome.message.clone());
                        target.exit_code = outcome.exit_code;
                        target
                            .started_at
                            .get_or_insert_with(|| completed_at.clone());
                        target.completed_at = Some(completed_at.clone());
                        target_terminalized = true;
                        targets
                            .iter()
                            .filter(|target| target.job_id == job_id)
                            .map(|target| target.status.clone())
                            .collect::<Vec<_>>()
                    };
                    memory.audits.write().await.push(AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: None,
                        action: "job.target_result".to_string(),
                        target: format!("client:{client_id}"),
                        command_hash: None,
                        metadata: serde_json::json!({
                            "job_id": job_id,
                            "status": outcome.status,
                            "exit_code": outcome.exit_code,
                            "accepted": outcome.accepted,
                            "message": outcome.message,
                            "received_at": outcome.received_at,
                        }),
                        created_at: completed_at.clone(),
                    });
                    if !statuses.is_empty()
                        && !statuses
                            .iter()
                            .any(|status| target_status_is_active(status))
                    {
                        let status = aggregate_job_status_from_statuses(&statuses, statuses.len())
                            .to_string();
                        if let Some(job) = memory
                            .jobs
                            .write()
                            .await
                            .iter_mut()
                            .find(|job| job.id == job_id && job.completed_at.is_none())
                        {
                            job.status = status.clone();
                            job.completed_at = Some(completed_at);
                            terminal_status = Some(status);
                        }
                    }
                }
                Ok(FinalJobOutputRecordResult {
                    write_result,
                    target_terminalized,
                })
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let (lock_a, lock_b) = append_lock_keys(job_id, client_id);
                sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
                    .bind(lock_a)
                    .bind(lock_b)
                    .execute(&mut *tx)
                    .await?;
                ensure_job_output_target_active_in_tx(&mut tx, job_id, client_id).await?;
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
                        accepted_persisted.push(stored_output.clone());
                        JobOutputWriteResult::DuplicateIdentical
                    }
                    Some(_) => {
                        if let Some(object_key) = stored_output.created_artifact_object_key.clone()
                        {
                            orphaned_object_keys.push(object_key);
                        }
                        insert_job_output_conflict_audit(&mut tx, &stored_output).await?;
                        JobOutputWriteResult::DuplicateConflict
                    }
                    None => {
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
                        accepted_persisted.push(stored_output.clone());
                        if let Some(artifact) =
                            job_output_server_artifact(client_id, &stored_output)
                        {
                            Repository::upsert_server_artifact_in_tx(&mut tx, &artifact, "active")
                                .await?;
                        }
                        JobOutputWriteResult::Inserted
                    }
                };
                let mut target_terminalized = false;
                if write_result != JobOutputWriteResult::DuplicateConflict {
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
                            last_dispatch_error = CASE WHEN $3 IN ('failed', 'control_timeout', 'agent_lost') THEN $4 ELSE NULL END
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
                    .execute(&mut *tx)
                    .await?;
                    if updated.rows_affected() == 0 {
                        anyhow::bail!("job_target_not_active");
                    }
                    target_terminalized = true;
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (
                            id, actor_id, action, target, command_hash, metadata
                        )
                        VALUES ($1, NULL, $2, $3, NULL, $4)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind("job.target_result")
                    .bind(format!("client:{client_id}"))
                    .bind(serde_json::json!({
                        "job_id": job_id,
                        "status": outcome.status,
                        "exit_code": outcome.exit_code,
                        "accepted": outcome.accepted,
                        "message": outcome.message,
                        "received_at": outcome.received_at,
                    }))
                    .execute(&mut *tx)
                    .await?;
                    terminal_status =
                        finish_job_in_tx_if_all_targets_terminal(&mut tx, job_id).await?;
                }
                tx.commit().await?;
                Ok(FinalJobOutputRecordResult {
                    write_result,
                    target_terminalized,
                })
            }
        };
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
        if !conflict_audits.is_empty() {
            if let Self::Memory(memory) = self {
                memory.audits.write().await.extend(conflict_audits);
            }
        }
        if let Some(store) = config.object_store {
            for object_key in orphaned_object_keys {
                store.delete_best_effort(&object_key).await;
            }
        }
        self.register_persisted_job_output_artifacts(client_id, &accepted_persisted)
            .await?;
        self.refresh_file_transfer_sessions_for_client(client_id)
            .await?;
        self.refresh_terminal_sessions_for_client(client_id).await?;
        if result.target_terminalized {
            self.record_job_target_webhook_event(job_id, client_id, outcome)
                .await?;
            if let Some(status) = terminal_status {
                self.record_job_terminal_side_effects(job_id, &status, None)
                    .await?;
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
            Self::Memory(memory) => {
                let stored = memory.job_outputs.read().await;
                let Some(existing) = stored.iter().find(|existing| {
                    existing.job_id == job_id
                        && existing.client_id == client_id
                        && existing.seq == seq
                }) else {
                    return Ok(None);
                };
                if job_output_view_matches_stored(existing, &expected) {
                    Ok(Some(JobOutputWriteResult::DuplicateIdentical))
                } else {
                    Ok(Some(JobOutputWriteResult::DuplicateConflict))
                }
            }
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

    pub(crate) async fn record_job_output_sequence_conflict_audit(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                memory
                    .audits
                    .write()
                    .await
                    .push(job_output_conflict_audit(job_id, client_id, seq));
            }
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
                }))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_job_outputs_checked_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        outputs: &[CommandOutput],
        config: JobOutputPersistConfig<'_>,
    ) -> Result<Vec<JobOutputWriteResult>> {
        self.record_job_outputs_starting_at(job_id, client_id, 0, outputs, None, config, false)
            .await
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) async fn append_job_output_chunk_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        output: &CommandOutput,
        config: JobOutputPersistConfig<'_>,
    ) -> Result<i32> {
        match self {
            Self::Memory(memory) => {
                let seq = memory
                    .job_outputs
                    .read()
                    .await
                    .iter()
                    .filter(|existing| existing.job_id == job_id && existing.client_id == client_id)
                    .map(|existing| existing.seq)
                    .max()
                    .unwrap_or(-1)
                    .saturating_add(1);
                self.record_job_output_chunk_with_config(
                    job_id, client_id, seq, output, None, config,
                )
                .await?;
                Ok(seq)
            }
            Self::Postgres(pool) => {
                let mut created_object_keys = Vec::new();
                let result: Result<i32> = async {
                    let mut tx = pool.begin().await?;
                    let (lock_a, lock_b) = append_lock_keys(job_id, client_id);
                    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
                        .bind(lock_a)
                        .bind(lock_b)
                        .execute(&mut *tx)
                        .await?;
                    let seq: i32 = sqlx::query_scalar(
                        r#"
                        SELECT COALESCE(MAX(seq), -1) + 1
                        FROM job_outputs
                        WHERE job_id = $1 AND client_id = $2
                        "#,
                    )
                    .bind(job_id)
                    .bind(client_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    let mut persisted = materialize_job_outputs(
                        job_id,
                        client_id,
                        seq,
                        std::slice::from_ref(output),
                        None,
                        config,
                    )
                    .await?;
                    created_object_keys.extend(
                        persisted
                            .iter()
                            .filter_map(|output| output.created_artifact_object_key.clone()),
                    );
                    let Some(output) = persisted.pop() else {
                        anyhow::bail!("append output materialization produced no rows");
                    };
                    sqlx::query(
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
                    if let Some(artifact) = job_output_server_artifact(client_id, &output) {
                        Repository::upsert_server_artifact_in_tx(&mut tx, &artifact, "active")
                            .await?;
                    }
                    tx.commit().await?;
                    Ok(seq)
                }
                .await;
                let seq = match result {
                    Ok(seq) => seq,
                    Err(error) => {
                        if let Some(store) = config.object_store {
                            for object_key in created_object_keys {
                                store.delete_best_effort(&object_key).await;
                            }
                        }
                        return Err(error);
                    }
                };
                self.record_network_observations_starting_at(
                    job_id,
                    client_id,
                    seq,
                    std::slice::from_ref(output),
                )
                .await?;
                if let Some(artifact) = self
                    .get_job_output_artifact_ref(job_id, client_id, seq)
                    .await?
                {
                    self.register_server_artifact(NewServerArtifact {
                        domain: "job_output".to_string(),
                        object_key: artifact.object_key,
                        sha256_hex: artifact.sha256_hex,
                        size_bytes: artifact.size_bytes,
                        job_id: Some(job_id),
                        client_id: Some(client_id.to_string()),
                        stream: Some(output_stream_name(output.stream).to_string()),
                        seq: Some(seq),
                        backup_request_id: None,
                        backup_artifact_id: None,
                        release_id: None,
                        metadata: serde_json::json!({}),
                    })
                    .await?;
                }
                self.refresh_file_transfer_sessions_for_client(client_id)
                    .await?;
                self.refresh_terminal_sessions_for_client(client_id).await?;
                Ok(seq)
            }
        }
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
        let mut accepted_persisted = Vec::new();
        let mut conflict_audits = Vec::new();
        let write_results: Vec<JobOutputWriteResult>;
        let result = match self {
            Self::Memory(memory) => {
                if require_active_target {
                    ensure_memory_job_output_target_active(memory, job_id, client_id).await?;
                }
                let mut stored = memory.job_outputs.write().await;
                let mut planned_results = Vec::with_capacity(persisted.len());
                let mut has_conflict = false;
                for output in &persisted {
                    if let Some(existing) = stored.iter().find(|existing| {
                        existing.job_id == output.job_id
                            && existing.client_id == output.client_id
                            && existing.seq == output.seq
                    }) {
                        if !job_output_view_matches_stored(existing, output) {
                            conflict_audits.push(job_output_conflict_audit(
                                output.job_id,
                                &output.client_id,
                                output.seq,
                            ));
                            planned_results.push(JobOutputWriteResult::DuplicateConflict);
                            has_conflict = true;
                        } else {
                            planned_results.push(JobOutputWriteResult::DuplicateIdentical);
                            accepted_persisted.push(output.clone());
                        }
                    } else {
                        planned_results.push(JobOutputWriteResult::Inserted);
                    }
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
                } else {
                    for (output, result) in persisted.iter().zip(planned_results.iter()) {
                        if *result == JobOutputWriteResult::Inserted {
                            stored.push(output.clone().into_view());
                            accepted_persisted.push(output.clone());
                        }
                    }
                }
                write_results = planned_results;
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let (lock_a, lock_b) = append_lock_keys(job_id, client_id);
                sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
                    .bind(lock_a)
                    .bind(lock_b)
                    .execute(&mut *tx)
                    .await?;
                if require_active_target {
                    ensure_job_output_target_active_in_tx(&mut tx, job_id, client_id).await?;
                }
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
                            accepted_persisted.push(output.clone());
                        }
                        Some(_) => {
                            planned_results.push(JobOutputWriteResult::DuplicateConflict);
                            conflict_outputs.push(output.clone());
                            has_conflict = true;
                            if let Some(object_key) = output.created_artifact_object_key.clone() {
                                orphaned_object_keys.push(object_key);
                            }
                        }
                        None => {
                            planned_results.push(JobOutputWriteResult::Inserted);
                        }
                    }
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
                        accepted_persisted.push(output.clone());
                        if let Some(artifact) = job_output_server_artifact(client_id, output) {
                            Repository::upsert_server_artifact_in_tx(&mut tx, &artifact, "active")
                                .await?;
                        }
                    }
                }
                write_results = planned_results;
                tx.commit().await
            }
        };
        if let Err(error) = result {
            if let Some(store) = config.object_store {
                for object_key in object_keys {
                    store.delete_best_effort(&object_key).await;
                }
            }
            return Err(error.into());
        }
        if !conflict_audits.is_empty() {
            if let Self::Memory(memory) = self {
                memory.audits.write().await.extend(conflict_audits);
            }
        }
        if let Some(store) = config.object_store {
            for object_key in orphaned_object_keys {
                store.delete_best_effort(&object_key).await;
            }
        }
        for (index, (output, write_result)) in outputs.iter().zip(write_results.iter()).enumerate()
        {
            if *write_result != JobOutputWriteResult::Inserted {
                continue;
            }
            let seq = start_seq
                .checked_add(i32::try_from(index)?)
                .ok_or_else(|| anyhow::anyhow!("job output sequence overflow"))?;
            self.record_network_observations_starting_at(
                job_id,
                client_id,
                seq,
                std::slice::from_ref(output),
            )
            .await?;
        }
        self.register_persisted_job_output_artifacts(client_id, &accepted_persisted)
            .await?;
        self.refresh_file_transfer_sessions_for_client(client_id)
            .await?;
        self.refresh_terminal_sessions_for_client(client_id).await?;
        Ok(write_results)
    }

    async fn register_persisted_job_output_artifacts(
        &self,
        client_id: &str,
        outputs: &[StoredJobOutput],
    ) -> Result<()> {
        for output in outputs {
            let Some(artifact) = job_output_server_artifact(client_id, output) else {
                continue;
            };
            self.register_server_artifact(artifact).await?;
        }
        Ok(())
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
    created_at: String,
}

fn job_output_server_artifact(
    client_id: &str,
    output: &StoredJobOutput,
) -> Option<NewServerArtifact> {
    Some(NewServerArtifact {
        domain: "job_output".to_string(),
        object_key: output.artifact_object_key.clone()?,
        sha256_hex: output.artifact_sha256_hex.clone()?,
        size_bytes: output.artifact_size_bytes?,
        job_id: Some(output.job_id),
        client_id: Some(client_id.to_string()),
        stream: Some(output.stream.clone()),
        seq: Some(output.seq),
        backup_request_id: None,
        backup_artifact_id: None,
        release_id: None,
        metadata: serde_json::json!({}),
    })
}

impl StoredJobOutput {
    fn into_view(self) -> JobOutputView {
        JobOutputView {
            job_id: self.job_id,
            client_id: self.client_id,
            seq: self.seq,
            stream: self.stream,
            data_base64: BASE64.encode(self.data),
            storage: self.storage,
            artifact_object_key: self.artifact_object_key,
            artifact_sha256_hex: self.artifact_sha256_hex,
            artifact_size_bytes: self.artifact_size_bytes,
            exit_code: self.exit_code,
            done: self.done,
            received_at: Some(self.received_at),
            created_at: self.created_at,
        }
    }
}

impl JobOutputView {
    fn into_list_item(self, include_data: bool) -> JobOutputListItemView {
        JobOutputListItemView {
            job_id: self.job_id,
            client_id: self.client_id,
            seq: self.seq,
            stream: self.stream,
            data_base64: include_data.then_some(self.data_base64),
            storage: self.storage,
            artifact_object_key: self.artifact_object_key,
            artifact_sha256_hex: self.artifact_sha256_hex,
            artifact_size_bytes: self.artifact_size_bytes,
            exit_code: self.exit_code,
            done: self.done,
            received_at: self.received_at,
            created_at: self.created_at,
        }
    }
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

fn job_output_view_matches_stored(existing: &JobOutputView, output: &StoredJobOutput) -> bool {
    existing.stream == output.stream
        && existing.data_base64 == BASE64.encode(&output.data)
        && existing.storage == output.storage
        && existing.artifact_object_key == output.artifact_object_key
        && existing.artifact_sha256_hex == output.artifact_sha256_hex
        && existing.artifact_size_bytes == output.artifact_size_bytes
        && existing.exit_code == output.exit_code
        && existing.done == output.done
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

async fn ensure_memory_job_output_target_active(
    memory: &MemoryState,
    job_id: Uuid,
    client_id: &str,
) -> Result<()> {
    let targets = memory.job_targets.read().await;
    let Some(target) = targets
        .iter()
        .find(|target| target.job_id == job_id && target.client_id == client_id)
    else {
        anyhow::bail!("job_target_not_found");
    };
    if target.completed_at.is_some() || !target_status_is_active(&target.status) {
        anyhow::bail!("job_target_not_active");
    }
    Ok(())
}

async fn ensure_job_output_target_active_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
    client_id: &str,
) -> Result<()> {
    let row = sqlx::query(
        r#"
        SELECT status, completed_at::text AS completed_at
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
    if completed_at.is_some() || !target_status_is_active(&status) {
        anyhow::bail!("job_target_not_active");
    }
    Ok(())
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
            created_at: String::new(),
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
            created_at: String::new(),
        })
    }
}

fn job_output_conflict_audit(job_id: Uuid, client_id: &str, seq: i32) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: None,
        action: "job.output_conflict_ignored".to_string(),
        target: format!("client:{client_id}"),
        command_hash: None,
        metadata: serde_json::json!({
            "job_id": job_id,
            "client_id": client_id,
            "seq": seq,
            "reason": "output sequence already persisted with different content",
        }),
        created_at: unix_now().to_string(),
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
    let created_at = unix_now().to_string();
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
                created_at: created_at.clone(),
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
                created_at: created_at.clone(),
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

fn build_process_supervisor_inventory(
    outputs: Vec<SupervisorInventoryOutput>,
    limit: i64,
) -> Vec<ProcessSupervisorInventoryView> {
    let mut seen = BTreeSet::<(String, String)>::new();
    let mut inventory = Vec::new();
    let limit = limit.clamp(1, 200) as usize;
    for output in outputs {
        for item in parse_process_supervisor_inventory_output(&output) {
            let key = (item.client_id.clone(), item.name.clone());
            if seen.insert(key) {
                inventory.push(item);
                if inventory.len() >= limit {
                    return inventory;
                }
            }
        }
    }
    inventory
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
mod tests {
    use super::{
        build_process_supervisor_inventory, JobOutputPersistConfig, JobOutputWriteResult,
        SupervisorInventoryOutput,
    };
    use crate::{object_store::BackupObjectStore, repository::MemoryState, Repository};
    use base64::Engine as _;
    use uuid::Uuid;
    use vpsman_common::{payload_hash, CommandOutput, OutputStream};

    #[tokio::test]
    async fn externalizes_large_non_status_outputs_to_object_store() {
        let repo = Repository::Memory(MemoryState::default());
        let root = std::env::temp_dir().join(format!("vpsman-job-output-store-{}", Uuid::new_v4()));
        let store = BackupObjectStore::filesystem(root.clone()).unwrap();
        let job_id = Uuid::new_v4();
        let data = b"large retained output".repeat(8);

        repo.record_job_outputs_with_config(
            job_id,
            "client-a",
            &[
                CommandOutput {
                    job_id,
                    stream: OutputStream::Stdout,
                    data: data.clone(),
                    exit_code: None,
                    done: false,
                },
                CommandOutput {
                    job_id,
                    stream: OutputStream::Status,
                    data: br#"{"type":"ok"}"#.to_vec(),
                    exit_code: Some(0),
                    done: true,
                },
            ],
            JobOutputPersistConfig {
                object_store: Some(&store),
                artifact_min_bytes: 16,
            },
        )
        .await
        .unwrap();

        let outputs = repo.list_job_outputs(job_id).await.unwrap();
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].storage, "object_store");
        assert_eq!(outputs[0].data_base64, super::BASE64.encode(&data));
        let expected_hash = payload_hash(&data);
        assert_eq!(
            outputs[0].artifact_sha256_hex.as_deref(),
            Some(expected_hash.as_str())
        );
        let artifact = repo
            .get_job_output_artifact_ref(job_id, "client-a", 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(store.get(&artifact.object_key).await.unwrap(), data);
        assert_eq!(outputs[1].storage, "inline");
        assert!(!outputs[1].data_base64.is_empty());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn rejects_oversized_status_output() {
        let repo = Repository::Memory(MemoryState::default());
        let job_id = Uuid::new_v4();
        let output = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: vec![b'x'; vpsman_server_core::STATUS_OUTPUT_MAX_BYTES + 1],
            exit_code: Some(1),
            done: true,
        };

        let error = repo
            .record_job_outputs_with_config(
                job_id,
                "client-a",
                &[output],
                JobOutputPersistConfig {
                    object_store: None,
                    artifact_min_bytes: usize::MAX,
                },
            )
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("status output exceeds max bytes"));
    }

    #[tokio::test]
    async fn incremental_output_recording_is_idempotent_by_sequence() {
        let repo = Repository::Memory(MemoryState::default());
        let job_id = Uuid::new_v4();
        let first = CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: b"hello".to_vec(),
            exit_code: None,
            done: false,
        };
        let done = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: br#"{"type":"ok"}"#.to_vec(),
            exit_code: Some(0),
            done: true,
        };

        repo.record_job_output_chunk_with_config(
            job_id,
            "client-a",
            0,
            &first,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();
        repo.record_job_outputs_with_config(
            job_id,
            "client-a",
            &[first, done],
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();

        let outputs = repo.list_job_outputs(job_id).await.unwrap();
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].seq, 0);
        assert_eq!(outputs[1].seq, 1);
        assert!(outputs[1].done);
    }

    #[tokio::test]
    async fn duplicate_conflicting_sequence_reports_conflict_without_replacing_output() {
        let repo = Repository::Memory(MemoryState::default());
        let job_id = Uuid::new_v4();
        let first = CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: b"first".to_vec(),
            exit_code: None,
            done: false,
        };
        let final_conflict = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: br#"{"type":"completed"}"#.to_vec(),
            exit_code: Some(0),
            done: true,
        };

        let inserted = repo
            .record_job_output_chunk_checked_with_config(
                job_id,
                "client-a",
                0,
                &first,
                None,
                JobOutputPersistConfig {
                    object_store: None,
                    artifact_min_bytes: usize::MAX,
                },
            )
            .await
            .unwrap();
        let duplicate = repo
            .record_job_output_chunk_checked_with_config(
                job_id,
                "client-a",
                0,
                &first,
                None,
                JobOutputPersistConfig {
                    object_store: None,
                    artifact_min_bytes: usize::MAX,
                },
            )
            .await
            .unwrap();
        let conflict = repo
            .record_job_output_chunk_checked_with_config(
                job_id,
                "client-a",
                0,
                &final_conflict,
                None,
                JobOutputPersistConfig {
                    object_store: None,
                    artifact_min_bytes: usize::MAX,
                },
            )
            .await
            .unwrap();

        assert_eq!(inserted, JobOutputWriteResult::Inserted);
        assert_eq!(duplicate, JobOutputWriteResult::DuplicateIdentical);
        assert_eq!(conflict, JobOutputWriteResult::DuplicateConflict);
        let outputs = repo.list_job_outputs(job_id).await.unwrap();
        assert_eq!(outputs.len(), 1);
        assert!(!outputs[0].done);
        assert_eq!(outputs[0].data_base64, super::BASE64.encode(b"first"));
        let audits = repo.list_audit_logs(10).await.unwrap();
        assert!(audits
            .iter()
            .any(|audit| audit.action == "job.output_conflict_ignored"));
    }

    #[tokio::test]
    async fn batch_conflict_poisons_later_final_output_insert() {
        let repo = Repository::Memory(MemoryState::default());
        let job_id = Uuid::new_v4();
        let first = CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: b"first".to_vec(),
            exit_code: None,
            done: false,
        };
        repo.record_job_output_chunk_with_config(
            job_id,
            "client-a",
            0,
            &first,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();

        let conflicting_replay = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: br#"{"type":"different"}"#.to_vec(),
            exit_code: Some(1),
            done: false,
        };
        let later_final = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: br#"{"type":"completed"}"#.to_vec(),
            exit_code: Some(0),
            done: true,
        };
        let results = repo
            .record_job_outputs_checked_with_config(
                job_id,
                "client-a",
                &[conflicting_replay, later_final],
                JobOutputPersistConfig {
                    object_store: None,
                    artifact_min_bytes: usize::MAX,
                },
            )
            .await
            .unwrap();

        assert!(results.contains(&JobOutputWriteResult::DuplicateConflict));
        let outputs = repo.list_job_outputs(job_id).await.unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].seq, 0);
        assert!(!outputs[0].done);
        assert_eq!(outputs[0].data_base64, super::BASE64.encode(b"first"));
    }

    #[tokio::test]
    async fn conflicting_replay_output_preserves_original_sequence_row() {
        let repo = Repository::Memory(MemoryState::default());
        let job_id = Uuid::new_v4();
        let original = CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: b"original output".to_vec(),
            exit_code: None,
            done: false,
        };
        let replay_conflict = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: br#"{"type":"duplicate_job_replayed"}"#.to_vec(),
            exit_code: Some(75),
            done: true,
        };

        repo.record_job_output_chunk_with_config(
            job_id,
            "client-a",
            0,
            &original,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();
        repo.record_job_output_chunk_with_config(
            job_id,
            "client-a",
            0,
            &replay_conflict,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();

        let outputs = repo.list_job_outputs(job_id).await.unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].stream, "stdout");
        assert_eq!(
            outputs[0].data_base64,
            super::BASE64.encode(b"original output")
        );
        let Repository::Memory(memory) = &repo else {
            unreachable!();
        };
        assert!(memory
            .audits
            .read()
            .await
            .iter()
            .any(|audit| audit.action == "job.output_conflict_ignored"
                && audit.metadata["seq"].as_i64() == Some(0)));
    }

    #[test]
    fn builds_deduplicated_supervisor_inventory_from_latest_outputs() {
        let start_job = Uuid::new_v4();
        let status_job = Uuid::new_v4();
        let outputs = vec![
            SupervisorInventoryOutput {
                job_id: status_job,
                client_id: "edge-a".to_string(),
                stream: "stdout".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "process_status",
                    "processes": [{
                        "name": "ospf-worker",
                        "status": "running",
                        "pid": 4242,
                        "started_unix": 1700000000_u64,
                        "stdout_log": "/tmp/ospf.stdout.log",
                        "stderr_log": "/tmp/ospf.stderr.log",
                        "restart_attempts": 2,
                        "last_exit_code": 7,
                        "last_exit_unix": 1700000010_u64,
                        "last_restart_unix": 1700000011_u64,
                        "limit_effectiveness": {
                            "overall": { "status": "degraded_desired_only" }
                        },
                        "cgroup_status": {
                            "status": "available",
                            "process_count": 2,
                            "cpu_weight": 39,
                            "memory_current_bytes": 1048576,
                            "pids_current": 2
                        }
                    }]
                }))
                .unwrap(),
                created_at: "200".to_string(),
                command_type: "process_status".to_string(),
            },
            SupervisorInventoryOutput {
                job_id: start_job,
                client_id: "edge-a".to_string(),
                stream: "status".to_string(),
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "process_start",
                    "name": "ospf-worker",
                    "status": "running",
                    "pid": 4000
                }))
                .unwrap(),
                created_at: "100".to_string(),
                command_type: "process_start".to_string(),
            },
        ];

        let inventory = build_process_supervisor_inventory(outputs, 50);

        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].client_id, "edge-a");
        assert_eq!(inventory[0].name, "ospf-worker");
        assert_eq!(inventory[0].pid, Some(4242));
        assert_eq!(inventory[0].source_job_id, status_job);
        assert_eq!(inventory[0].source_command_type, "process_status");
        assert_eq!(inventory[0].restart_attempts, Some(2));
        assert_eq!(inventory[0].last_exit_code, Some(7));
        assert_eq!(inventory[0].last_exit_unix, Some(1700000010));
        assert_eq!(inventory[0].last_restart_unix, Some(1700000011));
        assert_eq!(
            inventory[0].limit_effectiveness_status.as_deref(),
            Some("degraded_desired_only")
        );
        assert_eq!(inventory[0].cgroup_status.as_deref(), Some("available"));
        assert_eq!(inventory[0].cgroup_process_count, Some(2));
        assert_eq!(inventory[0].cgroup_cpu_weight, Some(39));
        assert_eq!(inventory[0].cgroup_memory_current_bytes, Some(1048576));
        assert_eq!(inventory[0].cgroup_pids_current, Some(2));
    }

    #[test]
    fn ignores_non_inventory_output_shapes() {
        let inventory = build_process_supervisor_inventory(
            vec![SupervisorInventoryOutput {
                job_id: Uuid::new_v4(),
                client_id: "edge-a".to_string(),
                stream: "stdout".to_string(),
                data: b"not json".to_vec(),
                created_at: "100".to_string(),
                command_type: "process_status".to_string(),
            }],
            50,
        );

        assert!(inventory.is_empty());
    }
}
