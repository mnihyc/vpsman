use vpsman_common::{
    decode_chunked_file_payload, decode_inline_file_payload, validate_absolute_file_path,
    validate_file_mode, validate_file_transfer_chunk_request,
    validate_file_transfer_download_chunk_request, validate_file_transfer_download_session,
    validate_file_transfer_session, validate_file_transfer_session_token, FilePushChunk,
    FileTransferValidationError, JobCommand,
};

use crate::ApiError;

pub(crate) fn validate_file_path(path: &str) -> Result<(), ApiError> {
    validate_absolute_file_path(path).map_err(file_transfer_error)
}

pub(crate) fn validate_file_push(
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    data_base64: &str,
) -> Result<(), ApiError> {
    validate_absolute_file_path(path).map_err(file_transfer_error)?;
    validate_file_mode(mode).map_err(file_transfer_error)?;
    decode_inline_file_payload(data_base64, size_bytes, sha256_hex).map_err(file_transfer_error)?;
    Ok(())
}

pub(crate) fn validate_chunked_file_push(
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    chunks: &[FilePushChunk],
) -> Result<(), ApiError> {
    validate_absolute_file_path(path).map_err(file_transfer_error)?;
    validate_file_mode(mode).map_err(file_transfer_error)?;
    decode_chunked_file_payload(chunks, size_bytes, sha256_hex).map_err(file_transfer_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_resumable_file_transfer_start(
    session_id: uuid::Uuid,
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: &str,
) -> Result<(), ApiError> {
    validate_file_transfer_session(
        session_id,
        path,
        mode,
        size_bytes,
        sha256_hex,
        chunk_size_bytes,
        rate_limit_kbps,
        resume_token_hash,
    )
    .map_err(file_transfer_error)
}

pub(crate) fn validate_resumable_file_transfer_chunk(
    session_id: uuid::Uuid,
    offset: u64,
    chunk: &FilePushChunk,
    resume_token_hash: &str,
) -> Result<(), ApiError> {
    validate_file_transfer_chunk_request(session_id, offset, chunk, resume_token_hash)
        .map(|_| ())
        .map_err(file_transfer_error)
}

pub(crate) fn validate_resumable_file_transfer_token(
    session_id: uuid::Uuid,
    resume_token_hash: &str,
) -> Result<(), ApiError> {
    validate_file_transfer_session_token(session_id, resume_token_hash).map_err(file_transfer_error)
}

pub(crate) fn validate_inline_file_payload(
    data_base64: &str,
    size_bytes: u64,
    sha256_hex: &str,
) -> Result<(), ApiError> {
    decode_inline_file_payload(data_base64, size_bytes, sha256_hex)
        .map(|_| ())
        .map_err(file_transfer_error)
}

pub(crate) fn file_command_type_label(command: &JobCommand) -> Option<&'static str> {
    Some(match command {
        JobCommand::FilePull { .. } => "file_pull",
        JobCommand::FilePush { .. } => "file_push",
        JobCommand::FilePushChunked { .. } => "file_push_chunked",
        JobCommand::FileTransferStart { .. } => "file_transfer_start",
        JobCommand::FileTransferChunk { .. } => "file_transfer_chunk",
        JobCommand::FileTransferCommit { .. } => "file_transfer_commit",
        JobCommand::FileTransferAbort { .. } => "file_transfer_abort",
        JobCommand::FileTransferDownloadStart { .. } => "file_transfer_download_start",
        JobCommand::FileTransferDownloadChunk { .. } => "file_transfer_download_chunk",
        _ => return None,
    })
}

pub(crate) fn validate_file_command(command: &JobCommand) -> Option<Result<(), ApiError>> {
    Some(match command {
        JobCommand::FilePull { path } => validate_file_path(path),
        JobCommand::FilePush {
            path,
            mode,
            size_bytes,
            sha256_hex,
            data_base64,
        } => validate_file_push(path, *mode, *size_bytes, sha256_hex, data_base64),
        JobCommand::FilePushChunked {
            path,
            mode,
            size_bytes,
            sha256_hex,
            chunks,
        } => validate_chunked_file_push(path, *mode, *size_bytes, sha256_hex, chunks),
        JobCommand::FileTransferStart {
            session_id,
            path,
            mode,
            size_bytes,
            sha256_hex,
            chunk_size_bytes,
            rate_limit_kbps,
            resume_token_hash,
        } => validate_resumable_file_transfer_start(
            *session_id,
            path,
            *mode,
            *size_bytes,
            sha256_hex,
            *chunk_size_bytes,
            *rate_limit_kbps,
            resume_token_hash,
        ),
        JobCommand::FileTransferChunk {
            session_id,
            offset,
            chunk,
            resume_token_hash,
        } => validate_resumable_file_transfer_chunk(*session_id, *offset, chunk, resume_token_hash),
        JobCommand::FileTransferCommit {
            session_id,
            resume_token_hash,
        }
        | JobCommand::FileTransferAbort {
            session_id,
            resume_token_hash,
        } => validate_resumable_file_transfer_token(*session_id, resume_token_hash),
        JobCommand::FileTransferDownloadStart {
            session_id,
            path,
            chunk_size_bytes,
            rate_limit_kbps,
            resume_token_hash,
        } => validate_resumable_file_transfer_download_start(
            *session_id,
            path,
            *chunk_size_bytes,
            *rate_limit_kbps,
            resume_token_hash,
        ),
        JobCommand::FileTransferDownloadChunk {
            session_id,
            offset,
            max_bytes,
            resume_token_hash,
        } => validate_resumable_file_transfer_download_chunk(
            *session_id,
            *offset,
            *max_bytes,
            resume_token_hash,
        ),
        _ => return None,
    })
}

pub(crate) fn validate_resumable_file_transfer_download_start(
    session_id: uuid::Uuid,
    path: &str,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: &str,
) -> Result<(), ApiError> {
    validate_file_transfer_download_session(
        session_id,
        path,
        chunk_size_bytes,
        rate_limit_kbps,
        resume_token_hash,
    )
    .map_err(file_transfer_error)
}

pub(crate) fn validate_resumable_file_transfer_download_chunk(
    session_id: uuid::Uuid,
    offset: u64,
    max_bytes: u32,
    resume_token_hash: &str,
) -> Result<(), ApiError> {
    validate_file_transfer_download_chunk_request(session_id, offset, max_bytes, resume_token_hash)
        .map_err(file_transfer_error)
}

fn file_transfer_error(error: FileTransferValidationError) -> ApiError {
    let code = match error {
        FileTransferValidationError::PathRequired => "file_path_required",
        FileTransferValidationError::InvalidPath => "invalid_file_path",
        FileTransferValidationError::PathMustBeAbsolute => "file_path_must_be_absolute",
        FileTransferValidationError::InvalidMode => "invalid_file_mode",
        FileTransferValidationError::TooLarge => "file_payload_too_large",
        FileTransferValidationError::InvalidBase64 => "file_payload_invalid_base64",
        FileTransferValidationError::SizeMismatch => "file_payload_size_mismatch",
        FileTransferValidationError::InvalidSha256 => "file_payload_invalid_sha256",
        FileTransferValidationError::HashMismatch => "file_payload_hash_mismatch",
        FileTransferValidationError::InvalidChunks => "file_payload_invalid_chunks",
        FileTransferValidationError::InvalidChunkOffset => "file_payload_invalid_chunk_offset",
        FileTransferValidationError::ChunkTooLarge => "file_payload_chunk_too_large",
        FileTransferValidationError::ChunkHashMismatch => "file_payload_chunk_hash_mismatch",
        FileTransferValidationError::InvalidSessionId => "file_transfer_invalid_session_id",
        FileTransferValidationError::InvalidResumeTokenHash => {
            "file_transfer_invalid_resume_token_hash"
        }
        FileTransferValidationError::InvalidChunkSize => "file_transfer_invalid_chunk_size",
        FileTransferValidationError::InvalidRateLimit => "file_transfer_invalid_rate_limit",
        FileTransferValidationError::InvalidOffset => "file_transfer_invalid_offset",
    };
    ApiError::bad_request(code)
}
