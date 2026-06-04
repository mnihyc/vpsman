use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
use vpsman_common::{payload_hash, CommandOutput, OutputStream};

use crate::model::{JobOutputView, ProcessSupervisorInventoryView};
use crate::object_store::BackupObjectStore;
use crate::repository::Repository;
use crate::{output_stream_name, unix_now};

const JOB_OUTPUT_ARTIFACT_PREFIX: &str = "job-outputs";

#[derive(Clone, Copy)]
pub(crate) struct JobOutputPersistConfig<'a> {
    pub(crate) object_store: Option<&'a BackupObjectStore>,
    pub(crate) artifact_min_bytes: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct JobOutputArtifactRef {
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
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
                            created_at: row.try_get("created_at")?,
                        })
                    })
                    .collect()
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
        self.record_job_outputs_starting_at(job_id, client_id, 0, outputs, config)
            .await
    }

    pub(crate) async fn record_job_output_chunk_with_config(
        &self,
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        output: &CommandOutput,
        config: JobOutputPersistConfig<'_>,
    ) -> Result<()> {
        self.record_job_outputs_starting_at(
            job_id,
            client_id,
            seq,
            std::slice::from_ref(output),
            config,
        )
        .await
    }

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
                self.record_job_output_chunk_with_config(job_id, client_id, seq, output, config)
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
                            done
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
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
                    .execute(&mut *tx)
                    .await?;
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
        config: JobOutputPersistConfig<'_>,
    ) -> Result<()> {
        if outputs.is_empty() {
            return Ok(());
        }
        let persisted =
            materialize_job_outputs(job_id, client_id, start_seq, outputs, config).await?;
        let object_keys = persisted
            .iter()
            .filter_map(|output| output.created_artifact_object_key.clone())
            .collect::<Vec<_>>();
        let result = match self {
            Self::Memory(memory) => {
                let mut stored = memory.job_outputs.write().await;
                for output in persisted {
                    let view = output.into_view();
                    if let Some(existing) = stored.iter_mut().find(|existing| {
                        existing.job_id == view.job_id
                            && existing.client_id == view.client_id
                            && existing.seq == view.seq
                    }) {
                        *existing = view;
                    } else {
                        stored.push(view);
                    }
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for output in &persisted {
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
                            done
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                        ON CONFLICT (job_id, client_id, seq)
                        DO UPDATE SET
                            stream = EXCLUDED.stream,
                            data = EXCLUDED.data,
                            storage = EXCLUDED.storage,
                            object_key = EXCLUDED.object_key,
                            data_sha256_hex = EXCLUDED.data_sha256_hex,
                            data_size_bytes = EXCLUDED.data_size_bytes,
                            exit_code = EXCLUDED.exit_code,
                            done = EXCLUDED.done
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
                    .execute(&mut *tx)
                    .await?;
                }
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
        self.record_network_observations_starting_at(job_id, client_id, start_seq, outputs)
            .await?;
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
    created_at: String,
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
            created_at: self.created_at,
        }
    }
}

async fn materialize_job_outputs(
    job_id: Uuid,
    client_id: &str,
    start_seq: i32,
    outputs: &[CommandOutput],
    config: JobOutputPersistConfig<'_>,
) -> Result<Vec<StoredJobOutput>> {
    let created_at = unix_now().to_string();
    let mut persisted = Vec::with_capacity(outputs.len());
    for (index, output) in outputs.iter().enumerate() {
        let seq = start_seq
            .checked_add(i32::try_from(index)?)
            .ok_or_else(|| anyhow::anyhow!("job output sequence overflow"))?;
        let should_externalize = should_externalize_output(output, &config);
        let stream = output_stream_name(output.stream).to_string();
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
                data: Vec::new(),
                storage: "object_store".to_string(),
                artifact_object_key: Some(object_key),
                created_artifact_object_key,
                artifact_sha256_hex: Some(sha256_hex),
                artifact_size_bytes: Some(output.data.len() as i64),
                exit_code: output.exit_code,
                done: output.done,
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
        Err(error) => match store.get(object_key).await {
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

fn append_lock_keys(job_id: Uuid, client_id: &str) -> (i32, i32) {
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
        build_process_supervisor_inventory, JobOutputPersistConfig, SupervisorInventoryOutput,
    };
    use crate::{object_store::BackupObjectStore, repository::MemoryState, Repository};
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
        assert_eq!(outputs[0].data_base64, "");
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
