use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::payload_hash;

pub const MAX_FILE_PATH_BYTES: usize = 4096;
pub const MAX_INLINE_FILE_PUSH_BYTES: usize = 1024 * 1024;
pub const FILE_TRANSFER_CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_CHUNKED_FILE_PUSH_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RESUMABLE_FILE_PUSH_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_RESUMABLE_FILE_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_FILE_TRANSFER_RATE_LIMIT_KBPS: u32 = 1_000_000;
pub const MAX_FILE_TRANSFER_RESUME_TOKEN_BYTES: usize = 128;
pub const MAX_FILE_TRANSFER_CHUNKS: usize =
    MAX_CHUNKED_FILE_PUSH_BYTES.div_ceil(FILE_TRANSFER_CHUNK_BYTES);
pub const MAX_FILE_MODE: u32 = 0o777;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FilePushChunk {
    pub offset: u64,
    pub size_bytes: u32,
    pub sha256_hex: String,
    pub data_base64: String,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum FileTransferValidationError {
    #[error("file path is required")]
    PathRequired,
    #[error("file path is invalid")]
    InvalidPath,
    #[error("file path must be absolute")]
    PathMustBeAbsolute,
    #[error("file mode is invalid")]
    InvalidMode,
    #[error("file size exceeds inline transfer limit")]
    TooLarge,
    #[error("file payload is invalid base64")]
    InvalidBase64,
    #[error("file payload size does not match declared size")]
    SizeMismatch,
    #[error("file sha256 must be a 64-character hex digest")]
    InvalidSha256,
    #[error("file payload hash mismatch")]
    HashMismatch,
    #[error("file chunk list is invalid")]
    InvalidChunks,
    #[error("file chunk offset is invalid")]
    InvalidChunkOffset,
    #[error("file chunk size exceeds limit")]
    ChunkTooLarge,
    #[error("file chunk hash mismatch")]
    ChunkHashMismatch,
    #[error("file transfer session id is invalid")]
    InvalidSessionId,
    #[error("file transfer resume token hash is invalid")]
    InvalidResumeTokenHash,
    #[error("file transfer chunk size is invalid")]
    InvalidChunkSize,
    #[error("file transfer rate limit is invalid")]
    InvalidRateLimit,
    #[error("file transfer offset is invalid")]
    InvalidOffset,
}

pub fn validate_absolute_file_path(path: &str) -> Result<(), FileTransferValidationError> {
    if path.is_empty() {
        return Err(FileTransferValidationError::PathRequired);
    }
    if path.len() > MAX_FILE_PATH_BYTES || path.as_bytes().contains(&0) {
        return Err(FileTransferValidationError::InvalidPath);
    }
    if !path.starts_with('/') {
        return Err(FileTransferValidationError::PathMustBeAbsolute);
    }
    Ok(())
}

pub fn validate_file_mode(mode: u32) -> Result<(), FileTransferValidationError> {
    if mode > MAX_FILE_MODE {
        return Err(FileTransferValidationError::InvalidMode);
    }
    Ok(())
}

pub fn normalize_sha256_hex(value: &str) -> Result<String, FileTransferValidationError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() != 64 || !normalized.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return Err(FileTransferValidationError::InvalidSha256);
    }
    Ok(normalized)
}

pub fn encode_inline_file_payload(data: &[u8]) -> Result<String, FileTransferValidationError> {
    if data.len() > MAX_INLINE_FILE_PUSH_BYTES {
        return Err(FileTransferValidationError::TooLarge);
    }
    Ok(STANDARD.encode(data))
}

pub fn decode_inline_file_payload(
    data_base64: &str,
    size_bytes: u64,
    sha256_hex: &str,
) -> Result<Vec<u8>, FileTransferValidationError> {
    if size_bytes > MAX_INLINE_FILE_PUSH_BYTES as u64 {
        return Err(FileTransferValidationError::TooLarge);
    }
    let max_base64_len = MAX_INLINE_FILE_PUSH_BYTES.div_ceil(3) * 4;
    if data_base64.len() > max_base64_len {
        return Err(FileTransferValidationError::TooLarge);
    }
    let expected_hash = normalize_sha256_hex(sha256_hex)?;
    let data = STANDARD
        .decode(data_base64)
        .map_err(|_| FileTransferValidationError::InvalidBase64)?;
    if data.len() as u64 != size_bytes {
        return Err(FileTransferValidationError::SizeMismatch);
    }
    if payload_hash(&data) != expected_hash {
        return Err(FileTransferValidationError::HashMismatch);
    }
    Ok(data)
}

pub fn encode_chunked_file_payload(
    data: &[u8],
) -> Result<Vec<FilePushChunk>, FileTransferValidationError> {
    if data.len() > MAX_CHUNKED_FILE_PUSH_BYTES {
        return Err(FileTransferValidationError::TooLarge);
    }
    Ok(data
        .chunks(FILE_TRANSFER_CHUNK_BYTES)
        .scan(0_u64, |offset, chunk| {
            let current = *offset;
            *offset += chunk.len() as u64;
            Some(FilePushChunk {
                offset: current,
                size_bytes: chunk.len() as u32,
                sha256_hex: payload_hash(chunk),
                data_base64: STANDARD.encode(chunk),
            })
        })
        .collect())
}

pub fn decode_chunked_file_payload(
    chunks: &[FilePushChunk],
    size_bytes: u64,
    sha256_hex: &str,
) -> Result<Vec<u8>, FileTransferValidationError> {
    if size_bytes > MAX_CHUNKED_FILE_PUSH_BYTES as u64 {
        return Err(FileTransferValidationError::TooLarge);
    }
    if chunks.len() > MAX_FILE_TRANSFER_CHUNKS {
        return Err(FileTransferValidationError::InvalidChunks);
    }
    if size_bytes > 0 && chunks.is_empty() {
        return Err(FileTransferValidationError::InvalidChunks);
    }
    let expected_hash = normalize_sha256_hex(sha256_hex)?;
    let mut data = Vec::with_capacity(size_bytes as usize);
    for chunk in chunks {
        if chunk.offset != data.len() as u64 {
            return Err(FileTransferValidationError::InvalidChunkOffset);
        }
        if chunk.size_bytes as usize > FILE_TRANSFER_CHUNK_BYTES {
            return Err(FileTransferValidationError::ChunkTooLarge);
        }
        let max_base64_len = FILE_TRANSFER_CHUNK_BYTES.div_ceil(3) * 4;
        if chunk.data_base64.len() > max_base64_len {
            return Err(FileTransferValidationError::ChunkTooLarge);
        }
        let chunk_hash = normalize_sha256_hex(&chunk.sha256_hex)?;
        let decoded = STANDARD
            .decode(&chunk.data_base64)
            .map_err(|_| FileTransferValidationError::InvalidBase64)?;
        if decoded.len() != chunk.size_bytes as usize {
            return Err(FileTransferValidationError::SizeMismatch);
        }
        if payload_hash(&decoded) != chunk_hash {
            return Err(FileTransferValidationError::ChunkHashMismatch);
        }
        if data.len().saturating_add(decoded.len()) > MAX_CHUNKED_FILE_PUSH_BYTES {
            return Err(FileTransferValidationError::TooLarge);
        }
        data.extend_from_slice(&decoded);
    }
    if data.len() as u64 != size_bytes {
        return Err(FileTransferValidationError::SizeMismatch);
    }
    if payload_hash(&data) != expected_hash {
        return Err(FileTransferValidationError::HashMismatch);
    }
    Ok(data)
}

#[allow(clippy::too_many_arguments)]
pub fn validate_file_transfer_session(
    session_id: uuid::Uuid,
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: &str,
) -> Result<(), FileTransferValidationError> {
    if session_id.is_nil() {
        return Err(FileTransferValidationError::InvalidSessionId);
    }
    validate_absolute_file_path(path)?;
    validate_file_mode(mode)?;
    if size_bytes > MAX_RESUMABLE_FILE_PUSH_BYTES {
        return Err(FileTransferValidationError::TooLarge);
    }
    normalize_sha256_hex(sha256_hex)?;
    validate_file_transfer_chunk_size(chunk_size_bytes)?;
    validate_file_transfer_rate_limit(rate_limit_kbps)?;
    validate_resume_token_hash(resume_token_hash)?;
    Ok(())
}

pub fn validate_file_transfer_chunk_request(
    session_id: uuid::Uuid,
    offset: u64,
    chunk: &FilePushChunk,
    resume_token_hash: &str,
) -> Result<Vec<u8>, FileTransferValidationError> {
    if session_id.is_nil() {
        return Err(FileTransferValidationError::InvalidSessionId);
    }
    if offset != chunk.offset {
        return Err(FileTransferValidationError::InvalidOffset);
    }
    validate_resume_token_hash(resume_token_hash)?;
    decode_file_transfer_chunk(chunk, FILE_TRANSFER_CHUNK_BYTES as u32)
}

pub fn validate_file_transfer_session_token(
    session_id: uuid::Uuid,
    resume_token_hash: &str,
) -> Result<(), FileTransferValidationError> {
    if session_id.is_nil() {
        return Err(FileTransferValidationError::InvalidSessionId);
    }
    validate_resume_token_hash(resume_token_hash)
}

pub fn validate_file_transfer_download_session(
    session_id: uuid::Uuid,
    path: &str,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: &str,
) -> Result<(), FileTransferValidationError> {
    if session_id.is_nil() {
        return Err(FileTransferValidationError::InvalidSessionId);
    }
    validate_absolute_file_path(path)?;
    validate_file_transfer_chunk_size(chunk_size_bytes)?;
    validate_file_transfer_rate_limit(rate_limit_kbps)?;
    validate_resume_token_hash(resume_token_hash)?;
    Ok(())
}

pub fn validate_file_transfer_download_chunk_request(
    session_id: uuid::Uuid,
    offset: u64,
    max_bytes: u32,
    resume_token_hash: &str,
) -> Result<(), FileTransferValidationError> {
    if session_id.is_nil() {
        return Err(FileTransferValidationError::InvalidSessionId);
    }
    if offset > MAX_RESUMABLE_FILE_DOWNLOAD_BYTES {
        return Err(FileTransferValidationError::InvalidOffset);
    }
    validate_file_transfer_chunk_size(max_bytes)?;
    validate_resume_token_hash(resume_token_hash)
}

pub fn decode_file_transfer_chunk(
    chunk: &FilePushChunk,
    max_chunk_bytes: u32,
) -> Result<Vec<u8>, FileTransferValidationError> {
    validate_file_transfer_chunk_size(max_chunk_bytes)?;
    if chunk.size_bytes == 0 || chunk.size_bytes > max_chunk_bytes {
        return Err(FileTransferValidationError::InvalidChunkSize);
    }
    let max_base64_len = (max_chunk_bytes as usize).div_ceil(3) * 4;
    if chunk.data_base64.len() > max_base64_len {
        return Err(FileTransferValidationError::ChunkTooLarge);
    }
    let chunk_hash = normalize_sha256_hex(&chunk.sha256_hex)?;
    let decoded = STANDARD
        .decode(&chunk.data_base64)
        .map_err(|_| FileTransferValidationError::InvalidBase64)?;
    if decoded.len() != chunk.size_bytes as usize {
        return Err(FileTransferValidationError::SizeMismatch);
    }
    if payload_hash(&decoded) != chunk_hash {
        return Err(FileTransferValidationError::ChunkHashMismatch);
    }
    Ok(decoded)
}

fn validate_file_transfer_chunk_size(value: u32) -> Result<(), FileTransferValidationError> {
    if value == 0 || value as usize > FILE_TRANSFER_CHUNK_BYTES {
        return Err(FileTransferValidationError::InvalidChunkSize);
    }
    Ok(())
}

fn validate_file_transfer_rate_limit(value: u32) -> Result<(), FileTransferValidationError> {
    if value > MAX_FILE_TRANSFER_RATE_LIMIT_KBPS {
        return Err(FileTransferValidationError::InvalidRateLimit);
    }
    Ok(())
}

fn validate_resume_token_hash(value: &str) -> Result<(), FileTransferValidationError> {
    normalize_sha256_hex(value)
        .map(|_| ())
        .map_err(|_| FileTransferValidationError::InvalidResumeTokenHash)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ABSOLUTE_FILE_PATH: &str = "/etc/hostname";

    #[test]
    fn validates_absolute_file_paths() {
        assert!(validate_absolute_file_path(TEST_ABSOLUTE_FILE_PATH).is_ok());
        assert_eq!(
            validate_absolute_file_path("relative").unwrap_err(),
            FileTransferValidationError::PathMustBeAbsolute
        );
        assert_eq!(
            validate_absolute_file_path("").unwrap_err(),
            FileTransferValidationError::PathRequired
        );
    }

    #[test]
    fn validates_inline_file_payload_hash_and_size() {
        let data = b"file contents";
        let encoded = encode_inline_file_payload(data).unwrap();
        let hash = payload_hash(data);
        assert_eq!(
            decode_inline_file_payload(&encoded, data.len() as u64, &hash).unwrap(),
            data
        );
        assert_eq!(
            decode_inline_file_payload(&encoded, data.len() as u64 + 1, &hash).unwrap_err(),
            FileTransferValidationError::SizeMismatch
        );
        assert_eq!(
            decode_inline_file_payload(&encoded, data.len() as u64, &"00".repeat(32)).unwrap_err(),
            FileTransferValidationError::HashMismatch
        );
    }

    #[test]
    fn validates_chunked_file_payload_offsets_and_hashes() {
        let data = vec![42_u8; FILE_TRANSFER_CHUNK_BYTES + 7];
        let chunks = encode_chunked_file_payload(&data).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].offset, FILE_TRANSFER_CHUNK_BYTES as u64);
        assert_eq!(
            decode_chunked_file_payload(&chunks, data.len() as u64, &payload_hash(&data)).unwrap(),
            data
        );

        let mut bad_offset = chunks.clone();
        bad_offset[1].offset += 1;
        assert_eq!(
            decode_chunked_file_payload(&bad_offset, data.len() as u64, &payload_hash(&data))
                .unwrap_err(),
            FileTransferValidationError::InvalidChunkOffset
        );

        let mut bad_chunk_hash = chunks;
        bad_chunk_hash[0].sha256_hex = "00".repeat(32);
        assert_eq!(
            decode_chunked_file_payload(&bad_chunk_hash, data.len() as u64, &payload_hash(&data))
                .unwrap_err(),
            FileTransferValidationError::ChunkHashMismatch
        );
    }

    #[test]
    fn validates_resumable_session_and_chunk_requests() {
        let data = b"resume chunk";
        let chunk = FilePushChunk {
            offset: 0,
            size_bytes: data.len() as u32,
            sha256_hex: payload_hash(data),
            data_base64: STANDARD.encode(data),
        };
        let session_id = uuid::Uuid::new_v4();
        let token_hash = payload_hash(b"resume-token");

        validate_file_transfer_session(
            session_id,
            "/tmp/resume.bin",
            0o600,
            data.len() as u64,
            &payload_hash(data),
            FILE_TRANSFER_CHUNK_BYTES as u32,
            0,
            &token_hash,
        )
        .unwrap();
        assert_eq!(
            validate_file_transfer_chunk_request(session_id, 0, &chunk, &token_hash).unwrap(),
            data
        );
        assert_eq!(
            validate_file_transfer_chunk_request(session_id, 1, &chunk, &token_hash).unwrap_err(),
            FileTransferValidationError::InvalidOffset
        );
        assert_eq!(
            validate_file_transfer_session(
                uuid::Uuid::nil(),
                "/tmp/resume.bin",
                0o600,
                data.len() as u64,
                &payload_hash(data),
                FILE_TRANSFER_CHUNK_BYTES as u32,
                0,
                &token_hash,
            )
            .unwrap_err(),
            FileTransferValidationError::InvalidSessionId
        );
    }

    #[test]
    fn validates_resumable_download_requests() {
        let session_id = uuid::Uuid::new_v4();
        let token_hash = payload_hash(b"download-token");

        validate_file_transfer_download_session(
            session_id,
            "/tmp/source.bin",
            FILE_TRANSFER_CHUNK_BYTES as u32,
            0,
            &token_hash,
        )
        .unwrap();
        validate_file_transfer_download_chunk_request(
            session_id,
            0,
            FILE_TRANSFER_CHUNK_BYTES as u32,
            &token_hash,
        )
        .unwrap();
        assert_eq!(
            validate_file_transfer_download_chunk_request(
                session_id,
                0,
                FILE_TRANSFER_CHUNK_BYTES as u32 + 1,
                &token_hash,
            )
            .unwrap_err(),
            FileTransferValidationError::InvalidChunkSize
        );
        assert_eq!(
            validate_file_transfer_download_session(
                uuid::Uuid::nil(),
                "/tmp/source.bin",
                FILE_TRANSFER_CHUNK_BYTES as u32,
                0,
                &token_hash,
            )
            .unwrap_err(),
            FileTransferValidationError::InvalidSessionId
        );
    }
}
