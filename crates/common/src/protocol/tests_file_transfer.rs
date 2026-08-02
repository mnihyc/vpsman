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
    for path in ["/tmp/.", "/tmp/..", "/tmp/../etc/passwd"] {
        assert_eq!(
            validate_absolute_file_path(path).unwrap_err(),
            FileTransferValidationError::InvalidPath,
            "{path}"
        );
    }
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
