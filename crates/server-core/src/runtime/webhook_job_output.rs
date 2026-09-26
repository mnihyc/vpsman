use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Context, Result};
use futures_util::TryStreamExt;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;
use vpsman_common::{payload_hash, template_referenced_paths};
use vpsman_object_store::BackupObjectStore;

// Read every sequence marker for selected targets, including status and
// unrequested streams, so missing chunks cannot masquerade as complete output.
// Only requested inline payloads are returned; external chunks are read from
// their authoritative objects. One ordered scan replaces per-chunk SQL reads.
const OUTPUT_SQL: &str = r#"
SELECT client_id, seq, stream, storage, object_key, data_sha256_hex, data_size_bytes,
       CASE WHEN storage = 'inline' AND (
           (stream = 'stdout' AND ($3 OR client_id = ANY($4::text[]))) OR
           (stream = 'stderr' AND ($5 OR client_id = ANY($6::text[])))
       ) THEN data ELSE NULL END AS data
FROM job_outputs
WHERE job_id = $1 AND ($2::text IS NULL OR client_id = $2)
  AND ($7 OR client_id = ANY($8::text[]))
ORDER BY client_id, seq
"#;

#[derive(Default)]
struct StreamSelection {
    all_clients: bool,
    clients: BTreeSet<String>,
}

impl StreamSelection {
    fn needed(&self) -> bool {
        self.all_clients || !self.clients.is_empty()
    }

    fn includes(&self, client_id: &str) -> bool {
        self.all_clients || self.clients.contains(client_id)
    }

    fn add_path(&mut self, suffix: &str) {
        if let Some(client_id) = suffix.strip_prefix('.') {
            if !client_id.is_empty() && !client_id.contains('.') {
                self.clients.insert(client_id.to_string());
                return;
            }
        }
        // A stream object or a less-specific reference needs its full value.
        // Do not guess a smaller source from an unfamiliar path.
        self.all_clients = true;
    }
}

fn output_selection(template: &str) -> Result<(StreamSelection, StreamSelection)> {
    let mut stdout = StreamSelection::default();
    let mut stderr = StreamSelection::default();
    for path in template_referenced_paths(template)? {
        if path == "job.output" {
            stdout.all_clients = true;
            stderr.all_clients = true;
        } else if let Some(suffix) = path.strip_prefix("job.output.stdout") {
            if suffix.is_empty() || suffix.starts_with('.') {
                stdout.add_path(suffix);
            }
        } else if let Some(suffix) = path.strip_prefix("job.output.stderr") {
            if suffix.is_empty() || suffix.starts_with('.') {
                stderr.add_path(suffix);
            }
        }
    }
    Ok((stdout, stderr))
}

/// Adds complete referenced retained streams to a temporary render context.
///
/// The caller authorizes jobs:read (the saved owner for automatic delivery).
/// Never persist this expanded context as the event or delivery payload. Helpers
/// operate on complete values; the template renderer, not this reader, limits
/// each rendered substitution. Reading a transformation's source therefore has
/// I/O and memory cost proportional to the selected retained output.
///
/// Target events expose stdout/stderr strings for exactly job.target.client_id.
/// Whole-job events expose stdout/stderr maps keyed by client ID. Missing streams
/// are empty strings/maps. Missing chunks or deleted artifacts are errors, never
/// silently substituted with an incomplete inline preview.
pub async fn enrich_webhook_job_output_context(
    connection: &mut PgConnection,
    context: &mut Value,
    template: &str,
    object_store: Option<&BackupObjectStore>,
) -> Result<()> {
    let (mut stdout, mut stderr) = output_selection(template)?;
    if !stdout.needed() && !stderr.needed() {
        return Ok(());
    }
    let Some(job) = context.get("job").and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(job_id) = job.get("id").and_then(Value::as_str) else {
        return Ok(());
    };
    let job_id = Uuid::parse_str(job_id).context("invalid webhook job identity")?;
    let target = match job.get("target") {
        Some(target) => Some(
            target
                .get("client_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .context("invalid webhook job target identity")?
                .to_string(),
        ),
        None => None,
    };
    if target.is_some() {
        // Target-event streams are strings, not maps. Their identity always
        // comes from the event, never a user-written placeholder suffix.
        stdout.all_clients = stdout.needed();
        stderr.all_clients = stderr.needed();
    }
    let stdout_clients = stdout.clients.iter().cloned().collect::<Vec<_>>();
    let stderr_clients = stderr.clients.iter().cloned().collect::<Vec<_>>();
    let selected_clients = stdout
        .clients
        .union(&stderr.clients)
        .cloned()
        .collect::<Vec<_>>();
    let mut rows = sqlx::query(OUTPUT_SQL)
        .bind(job_id)
        .bind(target.as_deref())
        .bind(stdout.all_clients)
        .bind(stdout_clients)
        .bind(stderr.all_clients)
        .bind(stderr_clients)
        .bind(stdout.all_clients || stderr.all_clients)
        .bind(selected_clients)
        .fetch(connection);
    let mut streams = BTreeMap::<(String, bool), Vec<u8>>::new();
    let mut current_client = None::<String>;
    let mut expected_seq = 0_i64;
    while let Some(row) = rows.try_next().await? {
        let client_id: String = row.try_get("client_id")?;
        if current_client.as_deref() != Some(client_id.as_str()) {
            current_client = Some(client_id.clone());
            expected_seq = 0;
        }
        let seq = i64::from(row.try_get::<i32, _>("seq")?);
        ensure!(
            seq == expected_seq,
            "retained job output is incomplete for target {client_id}"
        );
        expected_seq += 1;
        let stream: String = row.try_get("stream")?;
        let is_stderr = match stream.as_str() {
            "stdout" if stdout.includes(&client_id) => false,
            "stderr" if stderr.includes(&client_id) => true,
            _ => continue,
        };
        let storage: String = row.try_get("storage")?;
        let expected_size: Option<i64> = row.try_get("data_size_bytes")?;
        let expected_hash: Option<String> = row.try_get("data_sha256_hex")?;
        let data = match storage.as_str() {
            "inline" => row
                .try_get::<Option<Vec<u8>>, _>("data")?
                .context("retained inline job output is unavailable")?,
            "object_store" => {
                let store = object_store.context("job output object store is unavailable")?;
                let object_key = row
                    .try_get::<Option<String>, _>("object_key")?
                    .context("job output artifact identity is unavailable")?;
                let size = usize::try_from(
                    expected_size.context("job output artifact size is unavailable")?,
                )
                .context("invalid job output artifact size")?;
                ensure!(
                    expected_hash.is_some(),
                    "job output artifact hash is unavailable"
                );
                store.get_with_limit(&object_key, size).await?
            }
            "artifact_deleted" => anyhow::bail!("job output artifact was deleted"),
            _ => anyhow::bail!("unsupported job output storage: {storage}"),
        };
        if let Some(size) = expected_size {
            ensure!(
                i64::try_from(data.len()).ok() == Some(size),
                "job output artifact size mismatch"
            );
        }
        if let Some(hash) = expected_hash {
            ensure!(
                payload_hash(&data) == hash,
                "job output artifact hash mismatch"
            );
        }
        streams
            .entry((client_id, is_stderr))
            .or_default()
            .extend(data);
    }
    drop(rows);
    let mut stdout_values = Map::new();
    let mut stderr_values = Map::new();
    for ((client_id, is_stderr), bytes) in streams {
        // Decode after reassembling a stream: UTF-8 may span chunks separated by
        // status or the other output stream.
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
        };
        let values = if is_stderr {
            &mut stderr_values
        } else {
            &mut stdout_values
        };
        values.insert(client_id, Value::String(text));
    }
    context["job"]["output"] = if let Some(target) = target {
        json!({
            "stdout": stdout_values.remove(&target).unwrap_or_else(|| json!("")),
            "stderr": stderr_values.remove(&target).unwrap_or_else(|| json!("")),
        })
    } else {
        json!({"stdout": stdout_values, "stderr": stderr_values})
    };
    Ok(())
}

#[cfg(test)]
#[path = "tests_webhook_job_output.rs"]
mod tests;
