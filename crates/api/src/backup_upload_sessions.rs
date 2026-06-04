use std::{
    collections::HashMap,
    env,
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Mutex,
};
use uuid::Uuid;

use crate::{
    backup_handoff::MAX_BACKUP_ARTIFACT_CHUNKED_UPLOAD_BYTES,
    error::ApiError,
    model::{
        BackupArtifactUploadChunkRequest, BackupArtifactUploadCommitRequest,
        BackupArtifactUploadSessionCreateRequest, BackupArtifactUploadSessionView,
    },
    routes_backups::{
        validate_backup_artifact_object_key, validate_encrypted_backup_artifact_with_limit,
    },
    unix_now,
};

pub(crate) const BACKUP_ARTIFACT_UPLOAD_SESSION_TTL_SECS: u64 = 24 * 60 * 60;
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

#[derive(Clone, Debug)]
pub(crate) struct BackupArtifactUploadSessions {
    root: Arc<PathBuf>,
    sessions: Arc<Mutex<HashMap<Uuid, BackupArtifactUploadSession>>>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedBackupArtifactUpload {
    pub(crate) upload_id: Uuid,
    pub(crate) object_key: String,
    pub(crate) staging_path: PathBuf,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
}

#[derive(Clone, Debug)]
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
            sessions: Arc::new(Mutex::new(HashMap::new())),
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
        self.sessions.lock().await.insert(upload_id, session);
        Ok(view)
    }

    pub(crate) async fn write_chunk(
        &self,
        backup_request_id: Uuid,
        upload_id: Uuid,
        request: BackupArtifactUploadChunkRequest,
    ) -> Result<BackupArtifactUploadSessionView, ApiError> {
        let chunk = validate_backup_artifact_upload_chunk_request(&request)?;
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(&upload_id)
            .ok_or_else(|| ApiError::not_found("backup_artifact_upload_session_not_found"))?;
        ensure_session_matches_request(session, backup_request_id)?;
        ensure_session_not_expired(session)?;

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
        Ok(session.view())
    }

    pub(crate) async fn prepare_commit(
        &self,
        backup_request_id: Uuid,
        upload_id: Uuid,
        expected_client_id: &str,
        request: BackupArtifactUploadCommitRequest,
    ) -> Result<PreparedBackupArtifactUpload, ApiError> {
        if !request.confirmed {
            return Err(ApiError::conflict(
                "backup_artifact_upload_commit_confirmation_required",
            ));
        }

        let session = {
            let sessions = self.sessions.lock().await;
            let session = sessions
                .get(&upload_id)
                .ok_or_else(|| ApiError::not_found("backup_artifact_upload_session_not_found"))?;
            ensure_session_matches_request(session, backup_request_id)?;
            ensure_session_not_expired(session)?;
            if session.client_id != expected_client_id {
                return Err(ApiError::conflict("backup_artifact_upload_client_mismatch"));
            }
            if session.received_bytes != session.expected_size_bytes {
                return Err(ApiError::conflict("backup_artifact_upload_incomplete"));
            }
            session.clone()
        };

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
        let artifact = tokio::fs::read(&session.staging_path)
            .await
            .map_err(internal_error)?;
        validate_encrypted_backup_artifact_with_limit(
            &artifact,
            expected_client_id,
            MAX_BACKUP_ARTIFACT_CHUNKED_UPLOAD_BYTES,
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
        if let Some(session) = self.sessions.lock().await.remove(&upload_id) {
            let _ = tokio::fs::remove_file(session.staging_path).await;
        }
    }

    pub(crate) async fn abort(
        &self,
        backup_request_id: Uuid,
        upload_id: Uuid,
        confirmed: bool,
    ) -> Result<BackupArtifactUploadSessionView, ApiError> {
        if !confirmed {
            return Err(ApiError::conflict(
                "backup_artifact_upload_abort_confirmation_required",
            ));
        }
        let session = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions
                .get(&upload_id)
                .ok_or_else(|| ApiError::not_found("backup_artifact_upload_session_not_found"))?;
            ensure_session_matches_request(session, backup_request_id)?;
            sessions.remove(&upload_id).expect("checked session exists")
        };
        let view = session.view_with_status("aborted");
        let _ = tokio::fs::remove_file(session.staging_path).await;
        Ok(view)
    }

    fn staging_path(&self, upload_id: Uuid) -> PathBuf {
        self.root.join(format!("{upload_id}.part"))
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
    if !(1..=MAX_BACKUP_ARTIFACT_CHUNKED_UPLOAD_BYTES as i64).contains(&request.expected_size_bytes)
    {
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

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn internal_error(error: impl Into<anyhow::Error>) -> ApiError {
    ApiError::from(error.into())
}
