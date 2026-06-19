use vpsman_common::{
    decode_chunked_file_payload, decode_inline_file_payload, validate_absolute_file_path,
    validate_file_mode, validate_file_transfer_chunk_request,
    validate_file_transfer_download_chunk_request, validate_file_transfer_download_session,
    validate_file_transfer_session, validate_file_transfer_session_token, FileOwnershipPolicy,
    FilePushChunk, FileTransferValidationError, JobCommand, MAX_DIRECT_FILE_DOWNLOAD_BYTES,
    MAX_INLINE_FILE_PUSH_BYTES,
};

use crate::ApiError;

pub(crate) fn validate_file_path(path: &str) -> Result<(), ApiError> {
    validate_absolute_file_path(path).map_err(file_transfer_error)
}

pub(crate) fn validate_mutable_file_path(path: &str) -> Result<(), ApiError> {
    validate_file_path(path)?;
    if path == "/" {
        return Err(ApiError::bad_request("file_path_refuses_root_mutation"));
    }
    Ok(())
}

pub(crate) fn validate_file_push(
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    data_base64: &str,
) -> Result<(), ApiError> {
    validate_mutable_file_path(path)?;
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
    validate_mutable_file_path(path)?;
    validate_file_mode(mode).map_err(file_transfer_error)?;
    decode_chunked_file_payload(chunks, size_bytes, sha256_hex).map_err(file_transfer_error)?;
    Ok(())
}

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
    validate_mutable_file_path(path)?;
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

pub(crate) fn validate_file_command(command: &JobCommand) -> Option<Result<(), ApiError>> {
    Some(match command {
        JobCommand::FilePull { path } => validate_file_path(path),
        JobCommand::FilePush {
            path,
            mode,
            size_bytes,
            sha256_hex,
            data_base64,
            existing_policy: _,
            owner,
            group,
            uid,
            gid,
            ownership_policy,
        } => validate_file_push(path, *mode, *size_bytes, sha256_hex, data_base64).and_then(|_| {
            validate_ownership_request(
                owner.as_deref(),
                group.as_deref(),
                *uid,
                *gid,
                *ownership_policy,
            )
        }),
        JobCommand::FilePushChunked {
            path,
            mode,
            size_bytes,
            sha256_hex,
            chunks,
            existing_policy: _,
            owner,
            group,
            uid,
            gid,
            ownership_policy,
        } => validate_chunked_file_push(path, *mode, *size_bytes, sha256_hex, chunks).and_then(
            |_| {
                validate_ownership_request(
                    owner.as_deref(),
                    group.as_deref(),
                    *uid,
                    *gid,
                    *ownership_policy,
                )
            },
        ),
        JobCommand::FileTransferStart {
            session_id,
            path,
            mode,
            size_bytes,
            sha256_hex,
            chunk_size_bytes,
            rate_limit_kbps,
            existing_policy: _,
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
        JobCommand::FileStat { path } => validate_file_path(path),
        JobCommand::FileListDir {
            path,
            offset: _,
            limit,
            show_hidden: _,
        } => validate_file_list_dir(path, *limit),
        JobCommand::FileReadText {
            path, max_bytes, ..
        } => validate_file_read_text(path, *max_bytes),
        JobCommand::FileWriteText {
            path,
            mode,
            size_bytes,
            sha256_hex,
            content_base64,
            expected_sha256_hex,
            create: _,
            policy: _,
        } => validate_file_write_text(
            path,
            *mode,
            *size_bytes,
            sha256_hex,
            content_base64,
            expected_sha256_hex.as_deref(),
        ),
        JobCommand::FileMkdir {
            path,
            mode,
            recursive: _,
            policy: _,
        } => validate_file_mkdir(path, *mode),
        JobCommand::FileRename {
            path,
            new_path,
            overwrite: _,
            policy: _,
        } => validate_file_rename(path, new_path),
        JobCommand::FileDelete {
            path,
            recursive: _,
            policy: _,
        } => validate_mutable_file_path(path),
        JobCommand::FileChmod {
            path,
            mode,
            recursive: _,
            follow_symlinks: _,
            policy: _,
        } => validate_file_chmod(path, *mode),
        JobCommand::FileChown {
            path,
            owner,
            group,
            uid,
            gid,
            recursive: _,
            ownership_policy,
            policy: _,
        } => validate_file_path(path).and_then(|_| {
            validate_mutable_file_path(path)?;
            validate_ownership_request(
                owner.as_deref(),
                group.as_deref(),
                *uid,
                *gid,
                *ownership_policy,
            )
        }),
        JobCommand::FileCopy {
            path,
            new_path,
            overwrite: _,
            recursive: _,
            follow_symlinks: _,
            policy: _,
        } => validate_file_copy(path, new_path),
        JobCommand::FileDownload {
            path, max_bytes, ..
        } => validate_file_download(path, *max_bytes),
        JobCommand::FileArchiveTar {
            path, max_bytes, ..
        } => validate_file_archive_tar(path, *max_bytes),
        _ => return None,
    })
}

fn validate_file_list_dir(path: &str, limit: u32) -> Result<(), ApiError> {
    validate_file_path(path)?;
    if limit == 0 || limit > 1000 {
        return Err(ApiError::bad_request("file_list_limit_out_of_range"));
    }
    Ok(())
}

fn validate_file_read_text(path: &str, max_bytes: u64) -> Result<(), ApiError> {
    validate_file_path(path)?;
    if max_bytes == 0 || max_bytes > MAX_INLINE_FILE_PUSH_BYTES as u64 {
        return Err(ApiError::bad_request("file_read_max_bytes_out_of_range"));
    }
    Ok(())
}

fn validate_file_write_text(
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    content_base64: &str,
    expected_sha256_hex: Option<&str>,
) -> Result<(), ApiError> {
    validate_file_push(path, mode, size_bytes, sha256_hex, content_base64)?;
    if let Some(expected) = expected_sha256_hex {
        vpsman_common::normalize_sha256_hex(expected).map_err(file_transfer_error)?;
    }
    Ok(())
}

fn validate_file_mkdir(path: &str, mode: u32) -> Result<(), ApiError> {
    validate_mutable_file_path(path)?;
    validate_file_mode(mode).map_err(file_transfer_error)
}

fn validate_file_rename(path: &str, new_path: &str) -> Result<(), ApiError> {
    validate_mutable_file_path(path)?;
    validate_mutable_file_path(new_path)
}

fn validate_file_copy(path: &str, new_path: &str) -> Result<(), ApiError> {
    validate_mutable_file_path(path)?;
    validate_mutable_file_path(new_path)
}

fn validate_file_chmod(path: &str, mode: u32) -> Result<(), ApiError> {
    validate_mutable_file_path(path)?;
    validate_file_mode(mode).map_err(file_transfer_error)
}

fn validate_ownership_request(
    owner: Option<&str>,
    group: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    _ownership_policy: FileOwnershipPolicy,
) -> Result<(), ApiError> {
    if owner.map(invalid_owner_group_token).unwrap_or(false)
        || group.map(invalid_owner_group_token).unwrap_or(false)
    {
        return Err(ApiError::bad_request("invalid_owner_group"));
    }
    if owner.is_none() && group.is_none() && uid.is_none() && gid.is_none() {
        return Ok(());
    }
    Ok(())
}

fn invalid_owner_group_token(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty()
        || trimmed.len() > 128
        || trimmed
            .chars()
            .any(|character| character.is_control() || matches!(character, ':' | '/'))
}

fn validate_file_download(path: &str, max_bytes: u64) -> Result<(), ApiError> {
    validate_file_path(path)?;
    if max_bytes == 0 || max_bytes > MAX_DIRECT_FILE_DOWNLOAD_BYTES {
        return Err(ApiError::bad_request(
            "file_download_max_bytes_out_of_range",
        ));
    }
    Ok(())
}

fn validate_file_archive_tar(path: &str, max_bytes: u64) -> Result<(), ApiError> {
    validate_file_path(path)?;
    if max_bytes == 0 || max_bytes > MAX_DIRECT_FILE_DOWNLOAD_BYTES {
        return Err(ApiError::bad_request("file_archive_max_bytes_out_of_range"));
    }
    Ok(())
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
