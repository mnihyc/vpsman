use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    create_private_file_new_async, ensure_private_dir_async, payload_hash, AgentRuntimeConfig,
};

use crate::state_dir::agent_state_dir;

const CACHE_SCHEMA_VERSION: u16 = 1;
const CACHE_FILE_NAME: &str = "last-accepted.json";

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfigCache {
    root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RuntimeConfigCacheRecord {
    schema_version: u16,
    content_hash: String,
    config_json: String,
}

impl RuntimeConfigCache {
    pub(crate) async fn open_default() -> Result<Self> {
        Self::open_at(agent_state_dir()?.join("runtime-config")).await
    }

    pub(crate) async fn open_at(root: PathBuf) -> Result<Self> {
        ensure_private_dir_async(&root)
            .await
            .with_context(|| format!("failed to create runtime config cache {}", root.display()))?;
        Ok(Self { root })
    }

    pub(crate) async fn load(&self) -> Result<Option<AgentRuntimeConfig>> {
        let path = self.cache_path();
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to read runtime config cache {}", path.display())
                });
            }
        };
        let record: RuntimeConfigCacheRecord = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to decode runtime config cache {}", path.display()))?;
        anyhow::ensure!(
            record.schema_version == CACHE_SCHEMA_VERSION,
            "unsupported runtime config cache schema {}",
            record.schema_version
        );
        let observed_hash = payload_hash(record.config_json.as_bytes());
        anyhow::ensure!(
            observed_hash == record.content_hash,
            "runtime config cache hash mismatch"
        );
        let config = serde_json::from_str(&record.config_json).with_context(|| {
            format!("failed to decode cached runtime config {}", path.display())
        })?;
        Ok(Some(config))
    }

    pub(crate) async fn store(&self, config: &AgentRuntimeConfig) -> Result<()> {
        let config_json = serde_json::to_string(config)?;
        let record = RuntimeConfigCacheRecord {
            schema_version: CACHE_SCHEMA_VERSION,
            content_hash: payload_hash(config_json.as_bytes()),
            config_json,
        };
        let bytes = serde_json::to_vec(&record)?;
        let final_path = self.cache_path();
        let temp_path = self
            .root
            .join(format!(".{CACHE_FILE_NAME}.{}.tmp", Uuid::new_v4()));
        let mut file = create_private_file_new_async(&temp_path)
            .await
            .with_context(|| {
                format!(
                    "failed to create runtime config cache temp {}",
                    temp_path.display()
                )
            })?;
        if let Err(error) = file.write_all(&bytes).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(error).with_context(|| {
                format!(
                    "failed to write runtime config cache {}",
                    temp_path.display()
                )
            });
        }
        if let Err(error) = file.sync_all().await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(error).with_context(|| {
                format!(
                    "failed to fsync runtime config cache {}",
                    temp_path.display()
                )
            });
        }
        drop(file);
        if let Err(error) = tokio::fs::rename(&temp_path, &final_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(error).with_context(|| {
                format!(
                    "failed to promote runtime config cache {}",
                    final_path.display()
                )
            });
        }
        fsync_dir_best_effort(&self.root).await;
        Ok(())
    }

    fn cache_path(&self) -> PathBuf {
        self.root.join(CACHE_FILE_NAME)
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
        Ok(Err(error)) => warn!(%error, "failed to fsync runtime config cache directory"),
        Err(error) => warn!(%error, "failed to join runtime config cache fsync task"),
    }
}

#[cfg(test)]
#[path = "tests_runtime_config_cache.rs"]
mod tests;
