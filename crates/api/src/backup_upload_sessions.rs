use std::{
    collections::HashSet,
    env,
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    backup_handoff::backup_artifact_streaming_max_bytes,
    error::ApiError,
    model::{
        BackupArtifactUploadChunkRequest, BackupArtifactUploadCommitRequest,
        BackupArtifactUploadSessionCreateRequest, BackupArtifactUploadSessionView,
    },
    routes_backups::{
        validate_backup_artifact_object_key, validate_plain_backup_artifact_file_with_limit,
    },
    unix_now,
};

pub(crate) const BACKUP_ARTIFACT_UPLOAD_SESSION_TTL_SECS: u64 = 24 * 60 * 60;
const BACKUP_ARTIFACT_UPLOAD_CLEANUP_INTERVAL_SECS: u64 = 60 * 60;
pub(crate) const MAX_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES: usize = 4 * 1024 * 1024;

static BACKUP_UPLOAD_SESSIONS: OnceLock<BackupArtifactUploadSessions> = OnceLock::new();

pub(crate) fn backup_upload_sessions() -> &'static BackupArtifactUploadSessions {
    BACKUP_UPLOAD_SESSIONS.get_or_init(|| {
        let root = env::var_os("VPSMAN_BACKUP_UPLOAD_STAGING_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::temp_dir().join("vpsman-backup-upload-sessions"));
        BackupArtifactUploadSessions::new(root)
    })
}

pub(crate) fn spawn_backup_upload_session_cleanup() {
    let sessions = backup_upload_sessions().clone();
    tokio::spawn(async move {
        sessions.cleanup_expired().await;
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
            BACKUP_ARTIFACT_UPLOAD_CLEANUP_INTERVAL_SECS,
        ));
        loop {
            ticker.tick().await;
            sessions.cleanup_expired().await;
        }
    });
}

#[derive(Clone, Debug)]
pub(crate) struct BackupArtifactUploadSessions {
    root: Arc<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedBackupArtifactUpload {
    pub(crate) upload_id: Uuid,
    pub(crate) object_key: String,
    pub(crate) staging_path: PathBuf,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BackupArtifactUploadSession {
    upload_id: Uuid,
    backup_request_id: Uuid,
    client_id: String,
    object_key: String,
    expected_sha256_hex: String,
    expected_size_bytes: i64,
    received_bytes: i64,
    chunk_count: u64,
    created_unix: u64,
    updated_unix: u64,
    expires_unix: u64,
    staging_path: PathBuf,
}

impl BackupArtifactUploadSessions {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
        }
    }

    pub(crate) async fn create(
        &self,
        backup_request_id: Uuid,
        client_id: String,
        request: BackupArtifactUploadSessionCreateRequest,
    ) -> Result<BackupArtifactUploadSessionView, ApiError> {
        validate_backup_artifact_upload_session_create_request(&request)?;
        tokio::fs::create_dir_all(self.root.as_ref())
            .await
            .map_err(internal_error)?;
        self.cleanup_expired().await;

        let now = unix_now();
        let upload_id = Uuid::new_v4();
        let staging_path = self.staging_path(upload_id);
        let file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staging_path)
            .await
            .map_err(internal_error)?;
        drop(file);

        let session = BackupArtifactUploadSession {
            upload_id,
            backup_request_id,
            client_id,
            object_key: request.object_key,
            expected_sha256_hex: request.expected_sha256_hex.to_ascii_lowercase(),
            expected_size_bytes: request.expected_size_bytes,
            received_bytes: 0,
            chunk_count: 0,
            created_unix: now,
            updated_unix: now,
            expires_unix: now.saturating_add(BACKUP_ARTIFACT_UPLOAD_SESSION_TTL_SECS),
            staging_path,
        };
        let view = session.view();
        if let Err(error) = self.save_session(&session).await {
            let _ = tokio::fs::remove_file(&session.staging_path).await;
            return Err(error);
        }
        Ok(view)
    }

    pub(crate) async fn write_chunk(
        &self,
        backup_request_id: Uuid,
        upload_id: Uuid,
        request: BackupArtifactUploadChunkRequest,
    ) -> Result<BackupArtifactUploadSessionView, ApiError> {
        self.cleanup_expired().await;
        let chunk = validate_backup_artifact_upload_chunk_request(&request)?;
        let mut session = self.load_session(upload_id).await?;
        ensure_session_matches_request(&session, backup_request_id)?;
        ensure_session_not_expired(&session)?;

        let offset_bytes = request.offset_bytes;
        let chunk_len = i64::try_from(chunk.len())
            .map_err(|_| ApiError::bad_request("backup_artifact_upload_chunk_too_large"))?;
        let chunk_end = offset_bytes
            .checked_add(chunk_len)
            .ok_or_else(|| ApiError::bad_request("backup_artifact_upload_offset_invalid"))?;
        if chunk_end > session.expected_size_bytes {
            return Err(ApiError::bad_request(
                "backup_artifact_upload_size_exceeded",
            ));
        }

        if offset_bytes < session.received_bytes {
            if chunk_end > session.received_bytes {
                return Err(ApiError::conflict("backup_artifact_upload_offset_mismatch"));
            }
            ensure_retry_chunk_matches(&session.staging_path, offset_bytes, &chunk).await?;
            return Ok(session.view());
        }
        if offset_bytes != session.received_bytes {
            return Err(ApiError::conflict("backup_artifact_upload_offset_mismatch"));
        }

        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&session.staging_path)
            .await
            .map_err(internal_error)?;
        file.seek(SeekFrom::Start(offset_bytes as u64))
            .await
            .map_err(internal_error)?;
        file.write_all(&chunk).await.map_err(internal_error)?;
        file.flush().await.map_err(internal_error)?;

        session.received_bytes = chunk_end;
        session.chunk_count = session.chunk_count.saturating_add(1);
        session.updated_unix = unix_now();
        self.save_session(&session).await?;
        Ok(session.view())
    }

    pub(crate) async fn prepare_commit(
        &self,
        backup_request_id: Uuid,
        upload_id: Uuid,
        expected_client_id: &str,
        request: BackupArtifactUploadCommitRequest,
    ) -> Result<PreparedBackupArtifactUpload, ApiError> {
        self.cleanup_expired().await;
        if !request.confirmed {
            return Err(ApiError::conflict(
                "backup_artifact_upload_commit_confirmation_required",
            ));
        }

        let session = self.load_session(upload_id).await?;
        ensure_session_matches_request(&session, backup_request_id)?;
        ensure_session_not_expired(&session)?;
        if session.client_id != expected_client_id {
            return Err(ApiError::conflict("backup_artifact_upload_client_mismatch"));
        }
        if session.received_bytes != session.expected_size_bytes {
            return Err(ApiError::conflict("backup_artifact_upload_incomplete"));
        }

        let metadata = tokio::fs::metadata(&session.staging_path)
            .await
            .map_err(internal_error)?;
        if !metadata.is_file()
            || i64::try_from(metadata.len()).ok() != Some(session.expected_size_bytes)
        {
            return Err(ApiError::conflict(
                "backup_artifact_upload_staging_size_mismatch",
            ));
        }
        let sha256_hex = sha256_file_hex(&session.staging_path).await?;
        if sha256_hex != session.expected_sha256_hex {
            return Err(ApiError::conflict("backup_artifact_upload_sha256_mismatch"));
        }
        validate_plain_backup_artifact_file_with_limit(
            &session.staging_path,
            expected_client_id,
            backup_artifact_streaming_max_bytes(),
        )?;

        Ok(PreparedBackupArtifactUpload {
            upload_id,
            object_key: session.object_key,
            staging_path: session.staging_path,
            sha256_hex,
            size_bytes: session.expected_size_bytes,
        })
    }

    pub(crate) async fn finish(&self, upload_id: Uuid) {
        if let Ok(session) = self.load_session(upload_id).await {
            let _ = tokio::fs::remove_file(session.staging_path).await;
        }
        let _ = tokio::fs::remove_file(self.manifest_path(upload_id)).await;
    }

    pub(crate) async fn abort(
        &self,
        backup_request_id: Uuid,
        upload_id: Uuid,
        confirmed: bool,
    ) -> Result<BackupArtifactUploadSessionView, ApiError> {
        self.cleanup_expired().await;
        if !confirmed {
            return Err(ApiError::conflict(
                "backup_artifact_upload_abort_confirmation_required",
            ));
        }
        let session = self.load_session(upload_id).await?;
        ensure_session_matches_request(&session, backup_request_id)?;
        let view = session.view_with_status("aborted");
        let _ = tokio::fs::remove_file(session.staging_path).await;
        let _ = tokio::fs::remove_file(self.manifest_path(upload_id)).await;
        Ok(view)
    }

    fn staging_path(&self, upload_id: Uuid) -> PathBuf {
        self.root.join(format!("{upload_id}.part"))
    }

    fn manifest_path(&self, upload_id: Uuid) -> PathBuf {
        self.root.join(format!("{upload_id}.json"))
    }

    async fn load_session(&self, upload_id: Uuid) -> Result<BackupArtifactUploadSession, ApiError> {
        let manifest_path = self.manifest_path(upload_id);
        let bytes = tokio::fs::read(&manifest_path)
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => {
                    ApiError::not_found("backup_artifact_upload_session_not_found")
                }
                _ => internal_error(error),
            })?;
        serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::conflict("backup_artifact_upload_session_manifest_invalid"))
    }

    async fn save_session(&self, session: &BackupArtifactUploadSession) -> Result<(), ApiError> {
        let manifest_path = self.manifest_path(session.upload_id);
        let temp_path = manifest_path.with_extension(format!("json.tmp-{}", Uuid::new_v4()));
        let bytes = serde_json::to_vec(session).map_err(internal_error)?;
        tokio::fs::write(&temp_path, bytes)
            .await
            .map_err(internal_error)?;
        tokio::fs::rename(&temp_path, &manifest_path)
            .await
            .map_err(internal_error)?;
        Ok(())
    }

    async fn cleanup_expired(&self) {
        self.cleanup_expired_at(unix_now()).await;
    }

    async fn cleanup_expired_at(&self, now: u64) {
        let Ok(mut entries) = tokio::fs::read_dir(self.root.as_ref()).await else {
            return;
        };
        let mut retained_staging_paths = HashSet::<PathBuf>::new();
        let mut orphan_staging_candidates = Vec::<PathBuf>::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("part") {
                orphan_staging_candidates.push(path);
                continue;
            }
            if is_upload_session_temp_manifest(&path) {
                if file_is_stale(&path, now).await {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                continue;
            }
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            let Ok(session) = serde_json::from_slice::<BackupArtifactUploadSession>(&bytes) else {
                if file_is_stale(&path, now).await {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                continue;
            };
            let expected_staging_path = self.staging_path(session.upload_id);
            if session.expires_unix <= now {
                let _ = tokio::fs::remove_file(&expected_staging_path).await;
                let _ = tokio::fs::remove_file(&path).await;
            } else if session.staging_path == expected_staging_path {
                retained_staging_paths.insert(expected_staging_path);
            }
        }
        for path in orphan_staging_candidates {
            if !retained_staging_paths.contains(&path) && file_is_stale(&path, now).await {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
    }
}

impl BackupArtifactUploadSession {
    fn view(&self) -> BackupArtifactUploadSessionView {
        self.view_with_status(if self.received_bytes == self.expected_size_bytes {
            "uploaded"
        } else {
            "receiving"
        })
    }

    fn view_with_status(&self, status: &str) -> BackupArtifactUploadSessionView {
        BackupArtifactUploadSessionView {
            upload_id: self.upload_id,
            backup_request_id: self.backup_request_id,
            client_id: self.client_id.clone(),
            object_key: self.object_key.clone(),
            expected_sha256_hex: self.expected_sha256_hex.clone(),
            expected_size_bytes: self.expected_size_bytes,
            received_bytes: self.received_bytes,
            next_offset_bytes: self.received_bytes,
            chunk_count: self.chunk_count,
            max_chunk_bytes: MAX_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES,
            status: status.to_string(),
            created_unix: self.created_unix,
            updated_unix: self.updated_unix,
            expires_unix: self.expires_unix,
        }
    }
}

pub(crate) fn validate_backup_artifact_upload_session_create_request(
    request: &BackupArtifactUploadSessionCreateRequest,
) -> Result<(), ApiError> {
    validate_backup_artifact_object_key(&request.object_key)?;
    if !is_sha256_hex(&request.expected_sha256_hex) {
        return Err(ApiError::bad_request(
            "backup_artifact_upload_expected_sha256_invalid",
        ));
    }
    let max_size_bytes = i64::try_from(backup_artifact_streaming_max_bytes()).unwrap_or(i64::MAX);
    if !(1..=max_size_bytes).contains(&request.expected_size_bytes) {
        return Err(ApiError::bad_request(
            "backup_artifact_upload_expected_size_invalid",
        ));
    }
    if !request.confirmed {
        return Err(ApiError::conflict(
            "backup_artifact_upload_session_confirmation_required",
        ));
    }
    Ok(())
}

fn validate_backup_artifact_upload_chunk_request(
    request: &BackupArtifactUploadChunkRequest,
) -> Result<Vec<u8>, ApiError> {
    if request.offset_bytes < 0 {
        return Err(ApiError::bad_request(
            "backup_artifact_upload_offset_invalid",
        ));
    }
    if request.data_base64.trim().is_empty() {
        return Err(ApiError::bad_request(
            "backup_artifact_upload_chunk_required",
        ));
    }
    let max_base64_len = MAX_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES.div_ceil(3) * 4 + 16;
    if request.data_base64.len() > max_base64_len {
        return Err(ApiError::bad_request(
            "backup_artifact_upload_chunk_too_large",
        ));
    }
    let chunk = BASE64
        .decode(request.data_base64.trim())
        .map_err(|_| ApiError::bad_request("backup_artifact_upload_chunk_invalid"))?;
    if chunk.is_empty() {
        return Err(ApiError::bad_request(
            "backup_artifact_upload_chunk_required",
        ));
    }
    if chunk.len() > MAX_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES {
        return Err(ApiError::bad_request(
            "backup_artifact_upload_chunk_too_large",
        ));
    }
    Ok(chunk)
}

fn ensure_session_matches_request(
    session: &BackupArtifactUploadSession,
    backup_request_id: Uuid,
) -> Result<(), ApiError> {
    if session.backup_request_id == backup_request_id {
        Ok(())
    } else {
        Err(ApiError::conflict(
            "backup_artifact_upload_session_backup_mismatch",
        ))
    }
}

fn ensure_session_not_expired(session: &BackupArtifactUploadSession) -> Result<(), ApiError> {
    if unix_now() <= session.expires_unix {
        Ok(())
    } else {
        Err(ApiError::conflict("backup_artifact_upload_session_expired"))
    }
}

async fn ensure_retry_chunk_matches(
    path: &Path,
    offset_bytes: i64,
    chunk: &[u8],
) -> Result<(), ApiError> {
    let mut existing = vec![0_u8; chunk.len()];
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .await
        .map_err(internal_error)?;
    file.seek(SeekFrom::Start(offset_bytes as u64))
        .await
        .map_err(internal_error)?;
    file.read_exact(&mut existing)
        .await
        .map_err(internal_error)?;
    if existing == chunk {
        Ok(())
    } else {
        Err(ApiError::conflict(
            "backup_artifact_upload_retry_chunk_mismatch",
        ))
    }
}

async fn sha256_file_hex(path: &Path) -> Result<String, ApiError> {
    let mut file = tokio::fs::File::open(path).await.map_err(internal_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let bytes_read = file.read(&mut buffer).await.map_err(internal_error)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn is_upload_session_temp_manifest(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.contains(".json.tmp-"))
}

async fn file_is_stale(path: &Path, now: u64) -> bool {
    let Ok(metadata) = tokio::fs::symlink_metadata(path).await else {
        return false;
    };
    if !metadata.is_file() && !metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    let Ok(modified) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return false;
    };
    now.saturating_sub(modified.as_secs()) >= BACKUP_ARTIFACT_UPLOAD_SESSION_TTL_SECS
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn internal_error(error: impl Into<anyhow::Error>) -> ApiError {
    ApiError::from(error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_removes_expired_orphaned_and_temp_upload_files() {
        let root =
            std::env::temp_dir().join(format!("vpsman-backup-upload-cleanup-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let sessions = BackupArtifactUploadSessions::new(root.clone());
        let now = unix_now().saturating_add(BACKUP_ARTIFACT_UPLOAD_SESSION_TTL_SECS + 1);

        let live_id = Uuid::new_v4();
        let live_part = sessions.staging_path(live_id);
        let live_manifest = sessions.manifest_path(live_id);
        tokio::fs::write(&live_part, b"live").await.unwrap();
        let live = BackupArtifactUploadSession {
            upload_id: live_id,
            backup_request_id: Uuid::new_v4(),
            client_id: "edge-a".to_string(),
            object_key: "backups/edge-a/live.tar".to_string(),
            expected_sha256_hex: "a".repeat(64),
            expected_size_bytes: 4,
            received_bytes: 0,
            chunk_count: 0,
            created_unix: 1,
            updated_unix: 1,
            expires_unix: now.saturating_add(BACKUP_ARTIFACT_UPLOAD_SESSION_TTL_SECS),
            staging_path: live_part.clone(),
        };
        tokio::fs::write(&live_manifest, serde_json::to_vec(&live).unwrap())
            .await
            .unwrap();

        let expired_id = Uuid::new_v4();
        let expired_part = sessions.staging_path(expired_id);
        let expired_manifest = sessions.manifest_path(expired_id);
        tokio::fs::write(&expired_part, b"expired").await.unwrap();
        let expired = BackupArtifactUploadSession {
            upload_id: expired_id,
            backup_request_id: Uuid::new_v4(),
            client_id: "edge-a".to_string(),
            object_key: "backups/edge-a/expired.tar".to_string(),
            expected_sha256_hex: "b".repeat(64),
            expected_size_bytes: 7,
            received_bytes: 0,
            chunk_count: 0,
            created_unix: 1,
            updated_unix: 1,
            expires_unix: 1,
            staging_path: expired_part.clone(),
        };
        tokio::fs::write(&expired_manifest, serde_json::to_vec(&expired).unwrap())
            .await
            .unwrap();

        let orphan_part = sessions.staging_path(Uuid::new_v4());
        let temp_manifest = root.join(format!("{}.json.tmp-{}", Uuid::new_v4(), Uuid::new_v4()));
        tokio::fs::write(&orphan_part, b"orphan").await.unwrap();
        tokio::fs::write(&temp_manifest, b"partial").await.unwrap();

        sessions.cleanup_expired_at(now).await;

        assert!(live_part.exists());
        assert!(live_manifest.exists());
        assert!(!expired_part.exists());
        assert!(!expired_manifest.exists());
        assert!(!orphan_part.exists());
        assert!(!temp_manifest.exists());

        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
