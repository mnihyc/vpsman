use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    create_private_file_new_async, ensure_private_dir_async, payload_hash, CommandOutput,
    OutputStream, SequencedCommandOutput,
};

const LEDGER_SCHEMA_VERSION: u16 = 1;
const LEDGER_RETENTION_SECS: u64 = 72 * 60 * 60;
const LEDGER_MAX_ENTRIES: usize = 8192;
const LEDGER_MAX_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct CommandLedger {
    root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CommandLedgerEntry {
    pub(crate) schema_version: u16,
    pub(crate) job_id: Uuid,
    pub(crate) payload_hash: String,
    pub(crate) completed_unix: u64,
    pub(crate) terminal_output: Option<SequencedCommandOutput>,
    pub(crate) truncated: bool,
}

impl CommandLedger {
    pub(crate) async fn open_default() -> Result<Self> {
        let root = std::env::current_dir()
            .context("failed to resolve agent working directory")?
            .join("state")
            .join("command-ledger");
        Self::open_at(root).await
    }

    pub(crate) async fn open_at(root: PathBuf) -> Result<Self> {
        ensure_private_dir_async(&root)
            .await
            .with_context(|| format!("failed to create command ledger {}", root.display()))?;
        let ledger = Self { root };
        ledger.write_test().await?;
        ledger.cleanup().await?;
        Ok(ledger)
    }

    pub(crate) async fn lookup(&self, job_id: Uuid) -> Result<Option<CommandLedgerEntry>> {
        let path = self.entry_path(job_id);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read command ledger {}", path.display()));
            }
        };
        let entry: CommandLedgerEntry = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to decode command ledger {}", path.display()))?;
        if entry.schema_version != LEDGER_SCHEMA_VERSION || entry.job_id != job_id {
            return Ok(None);
        }
        Ok(Some(entry))
    }

    pub(crate) async fn record(
        &self,
        job_id: Uuid,
        payload_hash: String,
        terminal_output: Option<SequencedCommandOutput>,
        truncated: bool,
    ) -> Result<()> {
        let entry = CommandLedgerEntry {
            schema_version: LEDGER_SCHEMA_VERSION,
            job_id,
            payload_hash,
            completed_unix: unix_now(),
            terminal_output,
            truncated,
        };
        let final_path = self.entry_path(job_id);
        let temp_path = self.root.join(format!(".{job_id}.{}.tmp", Uuid::new_v4()));
        let bytes = serde_json::to_vec(&entry)?;
        let mut file = create_private_file_new_async(&temp_path)
            .await
            .with_context(|| {
                format!(
                    "failed to create command ledger temp {}",
                    temp_path.display()
                )
            })?;
        file.write_all(&bytes).await.with_context(|| {
            format!(
                "failed to write command ledger temp {}",
                temp_path.display()
            )
        })?;
        file.sync_all().await.with_context(|| {
            format!(
                "failed to fsync command ledger temp {}",
                temp_path.display()
            )
        })?;
        drop(file);
        tokio::fs::rename(&temp_path, &final_path)
            .await
            .with_context(|| {
                format!(
                    "failed to promote command ledger entry {}",
                    final_path.display()
                )
            })?;
        fsync_dir_best_effort(&self.root).await;
        self.cleanup().await?;
        Ok(())
    }

    fn entry_path(&self, job_id: Uuid) -> PathBuf {
        self.root.join(format!("{job_id}.json"))
    }

    async fn write_test(&self) -> Result<()> {
        let path = self
            .root
            .join(format!(".write-test-{}.tmp", Uuid::new_v4()));
        let mut file = create_private_file_new_async(&path)
            .await
            .with_context(|| {
                format!(
                    "failed to create command ledger write test {}",
                    path.display()
                )
            })?;
        file.write_all(b"ok").await.with_context(|| {
            format!(
                "failed to write command ledger write test {}",
                path.display()
            )
        })?;
        file.sync_all().await.with_context(|| {
            format!(
                "failed to fsync command ledger write test {}",
                path.display()
            )
        })?;
        drop(file);
        tokio::fs::remove_file(&path).await.with_context(|| {
            format!(
                "failed to remove command ledger write test {}",
                path.display()
            )
        })?;
        fsync_dir_best_effort(&self.root).await;
        Ok(())
    }

    async fn cleanup(&self) -> Result<()> {
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&self.root)
            .await
            .with_context(|| format!("failed to scan command ledger {}", self.root.display()))?;
        let now = unix_now();
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if !is_ledger_entry_file(&path) {
                continue;
            }
            let metadata = match entry.metadata().await {
                Ok(metadata) => metadata,
                Err(error) => {
                    warn!(%error, path = %path.display(), "failed to stat command ledger entry");
                    continue;
                }
            };
            let modified_unix = metadata
                .modified()
                .ok()
                .and_then(system_time_unix)
                .unwrap_or(now);
            if now.saturating_sub(modified_unix) > LEDGER_RETENTION_SECS {
                remove_ledger_entry(&path).await;
                continue;
            }
            entries.push((path, modified_unix, metadata.len()));
        }
        entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.0.cmp(&left.0)));
        let mut kept_entries = 0_usize;
        let mut kept_bytes = 0_u64;
        for (path, _, bytes) in entries {
            kept_entries = kept_entries.saturating_add(1);
            kept_bytes = kept_bytes.saturating_add(bytes);
            if kept_entries > LEDGER_MAX_ENTRIES || kept_bytes > LEDGER_MAX_BYTES {
                remove_ledger_entry(&path).await;
            }
        }
        Ok(())
    }
}

pub(crate) fn compact_ledger_terminal_output(
    output: Option<SequencedCommandOutput>,
) -> Option<SequencedCommandOutput> {
    output.map(|output| {
        let data = serde_json::to_vec(&serde_json::json!({
            "type": "duplicate_job_replay_unavailable",
            "status": if output.output.exit_code == Some(0) { "completed" } else { "failed" },
            "job_id": output.output.job_id,
            "reason": "command_ledger_replay",
            "original_stream": output_stream_name(output.output.stream),
            "original_data_size_bytes": output.output.data.len(),
            "original_data_sha256_hex": payload_hash(&output.output.data),
        }))
        .unwrap_or_else(|_| b"{\"type\":\"duplicate_job_replay_unavailable\"}".to_vec());
        SequencedCommandOutput {
            seq: output.seq,
            output: CommandOutput {
                job_id: output.output.job_id,
                stream: OutputStream::Status,
                data,
                exit_code: output.output.exit_code,
                done: true,
            },
        }
    })
}

fn output_stream_name(stream: OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
        OutputStream::Status => "status",
        OutputStream::Pty => "pty",
    }
}

fn is_ledger_entry_file(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("json")
        && path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_some()
}

async fn remove_ledger_entry(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            warn!(%error, path = %path.display(), "failed to remove command ledger entry");
        }
    }
}

async fn fsync_dir_best_effort(path: &Path) {
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        std::fs::File::open(&path).and_then(|file| file.sync_all())
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(%error, "failed to fsync command ledger directory"),
        Err(error) => warn!(%error, "failed to join command ledger directory fsync task"),
    }
}

fn system_time_unix(value: SystemTime) -> Option<u64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

fn unix_now() -> u64 {
    system_time_unix(SystemTime::now()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn command_ledger_records_and_loads_terminal_result() {
        let root =
            std::env::temp_dir().join(format!("vpsman-agent-command-ledger-{}", Uuid::new_v4()));
        let ledger = CommandLedger::open_at(root.clone()).await.unwrap();
        let job_id = Uuid::new_v4();
        let output = SequencedCommandOutput {
            seq: 3,
            output: CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: b"{\"type\":\"ok\"}".to_vec(),
                exit_code: Some(0),
                done: true,
            },
        };
        ledger
            .record(
                job_id,
                "a".repeat(64),
                compact_ledger_terminal_output(Some(output)),
                true,
            )
            .await
            .unwrap();
        let loaded = ledger.lookup(job_id).await.unwrap().unwrap();
        assert_eq!(loaded.job_id, job_id);
        assert_eq!(loaded.payload_hash, "a".repeat(64));
        assert!(loaded.terminal_output.unwrap().output.done);
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
