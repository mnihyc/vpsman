use std::collections::HashMap;

use axum::{
    body::to_bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

use crate::{
    gateway_client::GatewayDispatchClient,
    job_request::validate_job_command,
    model::{CreateJobRequest, HistoryQuery, JobHistoryView},
    model_file_transfer::{FileTransferHandoffRequest, UploadFileTransferSourceArtifactRequest},
    object_store::BackupObjectStore,
    repository::{MemoryState, Repository},
    repository_job_outputs::JobOutputPersistConfig,
    routes_file_transfers::{
        create_file_transfer_handoff, download_file_transfer_handoff,
        download_file_transfer_source_artifact, list_file_transfer_source_artifacts,
        upload_file_transfer_source_artifact,
    },
    routes_job_history::download_file_download_bundle,
    state::{AppState, EnrollmentSettings},
};
use vpsman_common::{
    encode_chunked_file_payload, encode_inline_file_payload, payload_hash, CommandOutput,
    FileActionPolicy, FileExistingPolicy, FileOwnershipPolicy, FilePushChunk, JobCommand,
    OutputStream, MAX_INLINE_FILE_PUSH_BYTES,
};

#[test]
fn validates_file_push_job_document() {
    let data = b"file contents";
    let command = JobCommand::FilePush {
        path: "/tmp/vpsman-upload.txt".to_string(),
        mode: 0o640,
        size_bytes: data.len() as u64,
        sha256_hex: payload_hash(data),
        data_base64: encode_inline_file_payload(data).unwrap(),
        existing_policy: FileExistingPolicy::Replace,
        owner: None,
        group: None,
        uid: None,
        gid: None,
        ownership_policy: FileOwnershipPolicy::Fail,
    };

    validate_job_command(&command).unwrap();
}

#[test]
fn rejects_invalid_file_push_job_document() {
    let data = b"file contents";
    let valid_data_base64 = encode_inline_file_payload(data).unwrap();
    let valid_hash = payload_hash(data);
    for command in [
        JobCommand::FilePush {
            path: "relative".to_string(),
            mode: 0o640,
            size_bytes: data.len() as u64,
            sha256_hex: valid_hash.clone(),
            data_base64: valid_data_base64.clone(),
            existing_policy: FileExistingPolicy::Replace,
            owner: None,
            group: None,
            uid: None,
            gid: None,
            ownership_policy: FileOwnershipPolicy::Fail,
        },
        JobCommand::FilePush {
            path: "/tmp/vpsman-upload.txt".to_string(),
            mode: 0o1000,
            size_bytes: data.len() as u64,
            sha256_hex: valid_hash.clone(),
            data_base64: valid_data_base64.clone(),
            existing_policy: FileExistingPolicy::Replace,
            owner: None,
            group: None,
            uid: None,
            gid: None,
            ownership_policy: FileOwnershipPolicy::Fail,
        },
        JobCommand::FilePush {
            path: "/tmp/vpsman-upload.txt".to_string(),
            mode: 0o640,
            size_bytes: data.len() as u64 + 1,
            sha256_hex: valid_hash.clone(),
            data_base64: valid_data_base64.clone(),
            existing_policy: FileExistingPolicy::Replace,
            owner: None,
            group: None,
            uid: None,
            gid: None,
            ownership_policy: FileOwnershipPolicy::Fail,
        },
        JobCommand::FilePush {
            path: "/tmp/vpsman-upload.txt".to_string(),
            mode: 0o640,
            size_bytes: data.len() as u64,
            sha256_hex: "00".repeat(32),
            data_base64: valid_data_base64.clone(),
            existing_policy: FileExistingPolicy::Replace,
            owner: None,
            group: None,
            uid: None,
            gid: None,
            ownership_policy: FileOwnershipPolicy::Fail,
        },
    ] {
        assert!(validate_job_command(&command).is_err(), "{command:?}");
    }
}

#[test]
fn rejects_unknown_file_operation_fields() {
    let command = serde_json::json!({
        "type": "file_copy",
        "path": "/tmp/source",
        "new_path": "/tmp/destination",
        "overwrite": false,
        "recursive": true,
        "policy": "fail",
        "overwite": true
    });
    assert!(serde_json::from_value::<JobCommand>(command).is_err());
}

#[test]
fn rejects_root_mutating_file_operations() {
    let data = b"file contents";
    let data_base64 = encode_inline_file_payload(data).unwrap();
    let sha256_hex = payload_hash(data);
    let commands = [
        JobCommand::FilePush {
            path: "/".to_string(),
            mode: 0o640,
            size_bytes: data.len() as u64,
            sha256_hex,
            data_base64,
            existing_policy: FileExistingPolicy::Replace,
            owner: None,
            group: None,
            uid: None,
            gid: None,
            ownership_policy: FileOwnershipPolicy::Fail,
        },
        JobCommand::FileDelete {
            path: "/".to_string(),
            recursive: true,
            policy: FileActionPolicy::Fail,
        },
        JobCommand::FileDelete {
            path: "/tmp/..".to_string(),
            recursive: true,
            policy: FileActionPolicy::Fail,
        },
        JobCommand::FileRename {
            path: "/tmp/source".to_string(),
            new_path: "/".to_string(),
            overwrite: true,
            policy: FileActionPolicy::Fail,
        },
        JobCommand::FileCopy {
            path: "/tmp/source".to_string(),
            new_path: "/".to_string(),
            overwrite: true,
            recursive: true,
            policy: FileActionPolicy::Fail,
        },
    ];
    for command in commands {
        assert!(validate_job_command(&command).is_err(), "{command:?}");
    }
}

fn transfer_job(id: uuid::Uuid, created_at: &str) -> JobHistoryView {
    JobHistoryView {
        id,
        actor_id: None,
        command_type: "file_transfer_download_chunk".to_string(),
        privileged: true,
        status: "completed".to_string(),
        target_count: 1,
        payload_hash: "aa".repeat(32),
        created_at: created_at.to_string(),
        completed_at: Some(created_at.to_string()),
    }
}

fn download_chunk_outputs(
    job_id: uuid::Uuid,
    session_id: uuid::Uuid,
    path: &str,
    offset: i64,
    chunk: &[u8],
    file_hash: &str,
    complete: bool,
) -> Vec<CommandOutput> {
    let next_offset = offset + chunk.len() as i64;
    vec![
        CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: chunk.to_vec(),
            exit_code: None,
            done: false,
        },
        CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "file_transfer_download_chunk",
                "session_id": session_id,
                "path": path,
                "next_offset": next_offset,
                "size_bytes": next_offset,
                "extra": {
                    "offset": offset,
                    "chunk_size_bytes": chunk.len(),
                    "chunk_sha256_hex": payload_hash(chunk),
                    "complete": complete,
                    "file_sha256_hex": file_hash,
                }
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        },
    ]
}

fn file_download_outputs(
    job_id: uuid::Uuid,
    path: &str,
    filename: &str,
    data: &[u8],
) -> Vec<CommandOutput> {
    vec![
        CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: data.to_vec(),
            exit_code: None,
            done: false,
        },
        CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "file_download",
                "status": "completed",
                "path": path,
                "source_kind": "file",
                "filename": filename,
                "content_type": "application/octet-stream",
                "size_bytes": data.len(),
                "sha256_hex": payload_hash(data),
                "archive": false,
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        },
    ]
}

fn file_download_outputs_with_hash(
    job_id: uuid::Uuid,
    path: &str,
    filename: &str,
    data: &[u8],
    sha256_hex: &str,
) -> Vec<CommandOutput> {
    vec![
        CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: data.to_vec(),
            exit_code: None,
            done: false,
        },
        CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "file_download",
                "status": "completed",
                "path": path,
                "source_kind": "file",
                "filename": filename,
                "content_type": "application/octet-stream",
                "size_bytes": data.len(),
                "sha256_hex": sha256_hex,
                "archive": false,
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        },
    ]
}

fn test_state_with_store(repo: Repository, store: BackupObjectStore) -> AppState {
    AppState {
        repo,
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        enrollment: EnrollmentSettings::default(),
        backup_object_store: Some(store),
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: usize::MAX,
        require_registered_agent_updates: false,
    }
}

#[test]
fn file_push_job_command_uses_operation_payload_and_type() {
    let data = b"file contents";
    let request = CreateJobRequest {
        selector_expression: "id:client-a".to_string(),
        destructive: false,
        confirmed: true,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FilePush {
            path: "/tmp/vpsman-upload.txt".to_string(),
            mode: 0o600,
            size_bytes: data.len() as u64,
            sha256_hex: payload_hash(data),
            data_base64: encode_inline_file_payload(data).unwrap(),
            existing_policy: FileExistingPolicy::Replace,
            owner: None,
            group: None,
            uid: None,
            gid: None,
            ownership_policy: FileOwnershipPolicy::Fail,
        }),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    assert_eq!(request.command_type_label(), "file_push");
    match request.job_command().unwrap() {
        JobCommand::FilePush {
            path,
            mode,
            size_bytes,
            ..
        } => {
            assert_eq!(path, "/tmp/vpsman-upload.txt");
            assert_eq!(mode, 0o600);
            assert_eq!(size_bytes, data.len() as u64);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn validates_chunked_file_push_job_document() {
    let data = vec![11_u8; MAX_INLINE_FILE_PUSH_BYTES + 17];
    let command = JobCommand::FilePushChunked {
        path: "/tmp/vpsman-upload.bin".to_string(),
        mode: 0o600,
        size_bytes: data.len() as u64,
        sha256_hex: payload_hash(&data),
        chunks: encode_chunked_file_payload(&data).unwrap(),
        existing_policy: FileExistingPolicy::Replace,
        owner: None,
        group: None,
        uid: None,
        gid: None,
        ownership_policy: FileOwnershipPolicy::Fail,
    };

    validate_job_command(&command).unwrap();
}

#[test]
fn rejects_invalid_chunked_file_push_job_document() {
    let data = vec![11_u8; MAX_INLINE_FILE_PUSH_BYTES + 17];
    let mut chunks = encode_chunked_file_payload(&data).unwrap();
    chunks[1].offset += 1;
    let command = JobCommand::FilePushChunked {
        path: "/tmp/vpsman-upload.bin".to_string(),
        mode: 0o600,
        size_bytes: data.len() as u64,
        sha256_hex: payload_hash(&data),
        chunks,
        existing_policy: FileExistingPolicy::Replace,
        owner: None,
        group: None,
        uid: None,
        gid: None,
        ownership_policy: FileOwnershipPolicy::Fail,
    };

    assert!(validate_job_command(&command).is_err());
}

#[test]
fn chunked_file_push_job_command_uses_operation_payload_and_type() {
    let data = vec![7_u8; MAX_INLINE_FILE_PUSH_BYTES + 17];
    let request = CreateJobRequest {
        selector_expression: "id:client-a".to_string(),
        destructive: false,
        confirmed: true,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FilePushChunked {
            path: "/tmp/vpsman-upload.bin".to_string(),
            mode: 0o600,
            size_bytes: data.len() as u64,
            sha256_hex: payload_hash(&data),
            chunks: encode_chunked_file_payload(&data).unwrap(),
            existing_policy: FileExistingPolicy::Replace,
            owner: None,
            group: None,
            uid: None,
            gid: None,
            ownership_policy: FileOwnershipPolicy::Fail,
        }),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    assert_eq!(request.command_type_label(), "file_push_chunked");
    match request.job_command().unwrap() {
        JobCommand::FilePushChunked {
            path,
            mode,
            size_bytes,
            chunks,
            ..
        } => {
            assert_eq!(path, "/tmp/vpsman-upload.bin");
            assert_eq!(mode, 0o600);
            assert_eq!(size_bytes, data.len() as u64);
            assert!(chunks.len() > 1);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn validates_resumable_file_transfer_job_documents() {
    let session_id = uuid::Uuid::new_v4();
    let token_hash = payload_hash(b"resume-token");
    let data = b"first chunk";
    let chunk = FilePushChunk {
        offset: 0,
        size_bytes: data.len() as u32,
        sha256_hex: payload_hash(data),
        data_base64: encode_inline_file_payload(data).unwrap(),
    };

    for command in [
        JobCommand::FileTransferStart {
            session_id,
            path: "/tmp/resumable.bin".to_string(),
            mode: 0o600,
            size_bytes: 128,
            sha256_hex: "11".repeat(32),
            chunk_size_bytes: 64 * 1024,
            rate_limit_kbps: 0,
            existing_policy: FileExistingPolicy::Replace,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferChunk {
            session_id,
            offset: 0,
            chunk: chunk.clone(),
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferCommit {
            session_id,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferAbort {
            session_id,
            resume_token_hash: token_hash.clone(),
        },
    ] {
        validate_job_command(&command).unwrap();
    }
}

#[test]
fn rejects_invalid_resumable_file_transfer_job_documents() {
    let session_id = uuid::Uuid::new_v4();
    let token_hash = payload_hash(b"resume-token");
    let data = b"first chunk";
    let chunk = FilePushChunk {
        offset: 0,
        size_bytes: data.len() as u32,
        sha256_hex: payload_hash(data),
        data_base64: encode_inline_file_payload(data).unwrap(),
    };
    let mut wrong_offset = chunk.clone();
    wrong_offset.offset = 1;

    for command in [
        JobCommand::FileTransferStart {
            session_id: uuid::Uuid::nil(),
            path: "/tmp/resumable.bin".to_string(),
            mode: 0o600,
            size_bytes: 128,
            sha256_hex: "11".repeat(32),
            chunk_size_bytes: 64 * 1024,
            rate_limit_kbps: 0,
            existing_policy: FileExistingPolicy::Replace,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferStart {
            session_id,
            path: "relative.bin".to_string(),
            mode: 0o600,
            size_bytes: 128,
            sha256_hex: "11".repeat(32),
            chunk_size_bytes: 64 * 1024,
            rate_limit_kbps: 0,
            existing_policy: FileExistingPolicy::Replace,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferStart {
            session_id,
            path: "/tmp/resumable.bin".to_string(),
            mode: 0o600,
            size_bytes: 128,
            sha256_hex: "11".repeat(32),
            chunk_size_bytes: 0,
            rate_limit_kbps: 0,
            existing_policy: FileExistingPolicy::Replace,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferStart {
            session_id,
            path: "/tmp/resumable.bin".to_string(),
            mode: 0o600,
            size_bytes: 128,
            sha256_hex: "11".repeat(32),
            chunk_size_bytes: 64 * 1024,
            rate_limit_kbps: 1_000_001,
            existing_policy: FileExistingPolicy::Replace,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferChunk {
            session_id,
            offset: 0,
            chunk: wrong_offset,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferCommit {
            session_id,
            resume_token_hash: "not-hex".to_string(),
        },
    ] {
        assert!(validate_job_command(&command).is_err(), "{command:?}");
    }
}

#[tokio::test]
async fn file_transfer_handoff_assembles_completed_download_from_retained_outputs() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let store_root = std::env::temp_dir().join(format!(
        "vpsman-transfer-handoff-store-{}",
        uuid::Uuid::new_v4()
    ));
    let store = BackupObjectStore::filesystem(store_root.clone()).unwrap();
    let state = test_state_with_store(repo.clone(), store.clone());
    let client_id = "edge-a";
    let session_id = uuid::Uuid::new_v4();
    let first = b"hello ".to_vec();
    let second = b"world".to_vec();
    let mut all = first.clone();
    all.extend_from_slice(&second);
    let file_hash = payload_hash(&all);
    let first_job = uuid::Uuid::parse_str("11111111-2222-4333-8444-000000000001").unwrap();
    let second_job = uuid::Uuid::parse_str("22222222-2222-4333-8444-000000000002").unwrap();

    memory.jobs.write().await.extend([
        transfer_job(first_job, "100"),
        transfer_job(second_job, "200"),
    ]);
    repo.record_job_outputs_with_config(
        first_job,
        client_id,
        &download_chunk_outputs(
            first_job,
            session_id,
            "/tmp/app.log",
            0,
            &first,
            &file_hash,
            false,
        ),
        JobOutputPersistConfig {
            object_store: None,
            artifact_min_bytes: usize::MAX,
        },
    )
    .await
    .unwrap();
    repo.record_job_outputs_with_config(
        second_job,
        client_id,
        &download_chunk_outputs(
            second_job,
            session_id,
            "/tmp/app.log",
            first.len() as i64,
            &second,
            &file_hash,
            true,
        ),
        JobOutputPersistConfig {
            object_store: Some(&store),
            artifact_min_bytes: 1,
        },
    )
    .await
    .unwrap();

    let Json(handoff) = create_file_transfer_handoff(
        State(state.clone()),
        HeaderMap::new(),
        Path((client_id.to_string(), session_id)),
        Json(FileTransferHandoffRequest { confirmed: true }),
    )
    .await
    .unwrap();

    assert_eq!(handoff.client_id, client_id);
    assert_eq!(handoff.session_id, session_id);
    assert_eq!(handoff.sha256_hex, file_hash);
    assert_eq!(handoff.size_bytes, all.len() as i64);
    assert_eq!(handoff.chunk_count, 2);
    assert!(handoff.object_key.starts_with("file-transfers/"));

    let response = download_file_transfer_handoff(
        State(state),
        HeaderMap::new(),
        Path((client_id.to_string(), session_id)),
    )
    .await
    .unwrap();
    assert_eq!(
        response
            .headers()
            .get("x-vpsman-artifact-delivery")
            .unwrap()
            .to_str()
            .unwrap(),
        "streamed-filesystem"
    );
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .unwrap()
            .to_str()
            .unwrap(),
        all.len().to_string()
    );
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(body.as_ref(), all.as_slice());
    let stored = store.get(&handoff.object_key).await.unwrap();
    assert_eq!(stored, all);
    let _ = tokio::fs::remove_dir_all(store_root).await;
}

#[tokio::test]
async fn file_download_bundle_route_returns_tar_archive_from_target_outputs() {
    let repo = Repository::Memory(MemoryState::default());
    let store_root = std::env::temp_dir().join(format!(
        "vpsman-file-download-bundle-store-{}",
        uuid::Uuid::new_v4()
    ));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(store_root.clone()).unwrap(),
    );
    let job_id = uuid::Uuid::new_v4();

    repo.record_job_outputs(
        job_id,
        "edge-a",
        &file_download_outputs(job_id, "/etc/app.conf", "app.conf", b"listen=443\n"),
    )
    .await
    .unwrap();
    repo.record_job_outputs(
        job_id,
        "edge-b",
        &file_download_outputs(job_id, "/etc/app.conf", "app.conf", b"listen=8443\n"),
    )
    .await
    .unwrap();

    let response = download_file_download_bundle(
        State(state),
        HeaderMap::new(),
        Path(job_id),
        Query(crate::routes_job_history::FileDownloadBundleQuery { clients: None }),
    )
    .await
    .unwrap();

    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/x-tar"
    );
    assert_eq!(
        response
            .headers()
            .get("x-vpsman-artifact-delivery")
            .unwrap()
            .to_str()
            .unwrap(),
        "spooled-filesystem"
    );
    assert!(response
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .is_some());
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let mut archive = tar::Archive::new(std::io::Cursor::new(body));
    let mut entries = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
        entries.push((path, bytes));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        entries,
        vec![
            ("edge-a/app.conf".to_string(), b"listen=443\n".to_vec()),
            ("edge-b/app.conf".to_string(), b"listen=8443\n".to_vec()),
        ]
    );
    let _ = tokio::fs::remove_dir_all(store_root).await;
}

#[tokio::test]
async fn file_download_bundle_route_handles_more_than_twenty_target_outputs() {
    let repo = Repository::Memory(MemoryState::default());
    let store_root = std::env::temp_dir().join(format!(
        "vpsman-file-download-bundle-many-store-{}",
        uuid::Uuid::new_v4()
    ));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(store_root.clone()).unwrap(),
    );
    let job_id = uuid::Uuid::new_v4();

    for index in 0..24 {
        let client_id = format!("edge-{index:02}");
        let data = format!("client={client_id}\n");
        repo.record_job_outputs(
            job_id,
            &client_id,
            &file_download_outputs(job_id, "/var/log/app.log", "app.log", data.as_bytes()),
        )
        .await
        .unwrap();
    }

    let response = download_file_download_bundle(
        State(state),
        HeaderMap::new(),
        Path(job_id),
        Query(crate::routes_job_history::FileDownloadBundleQuery { clients: None }),
    )
    .await
    .unwrap();

    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let mut archive = tar::Archive::new(std::io::Cursor::new(body));
    let mut entries = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
        entries.push((path, bytes));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(entries.len(), 24);
    assert_eq!(
        entries.first().unwrap(),
        &("edge-00/app.log".to_string(), b"client=edge-00\n".to_vec())
    );
    assert_eq!(
        entries.last().unwrap(),
        &("edge-23/app.log".to_string(), b"client=edge-23\n".to_vec())
    );
    let _ = tokio::fs::remove_dir_all(store_root).await;
}

#[tokio::test]
async fn file_download_bundle_route_rejects_output_integrity_mismatch() {
    let repo = Repository::Memory(MemoryState::default());
    let store_root = std::env::temp_dir().join(format!(
        "vpsman-file-download-bundle-mismatch-store-{}",
        uuid::Uuid::new_v4()
    ));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(store_root.clone()).unwrap(),
    );
    let job_id = uuid::Uuid::new_v4();

    repo.record_job_outputs(
        job_id,
        "edge-a",
        &file_download_outputs_with_hash(
            job_id,
            "/etc/app.conf",
            "app.conf",
            b"listen=443\n",
            &"00".repeat(32),
        ),
    )
    .await
    .unwrap();

    let result = download_file_download_bundle(
        State(state),
        HeaderMap::new(),
        Path(job_id),
        Query(crate::routes_job_history::FileDownloadBundleQuery { clients: None }),
    )
    .await;
    let error = result.expect_err("bundle route should reject mismatched output");

    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "file_download_output_integrity_mismatch");
    let _ = tokio::fs::remove_dir_all(store_root).await;
}

#[tokio::test]
async fn file_transfer_source_artifact_upload_records_and_serves_verified_object() {
    let repo = Repository::Memory(MemoryState::default());
    let store_root = std::env::temp_dir().join(format!(
        "vpsman-transfer-source-store-{}",
        uuid::Uuid::new_v4()
    ));
    let store = BackupObjectStore::filesystem(store_root.clone()).unwrap();
    let state = test_state_with_store(repo.clone(), store.clone());
    let payload = b"source artifact bytes for repeated upload".to_vec();
    let sha256_hex = payload_hash(&payload);

    let (status, Json(artifact)) = upload_file_transfer_source_artifact(
        State(state.clone()),
        HeaderMap::new(),
        Json(UploadFileTransferSourceArtifactRequest {
            name: Some("../source.bin".to_string()),
            source_base64: BASE64.encode(&payload),
            sha256_hex: sha256_hex.clone(),
            size_bytes: payload.len() as i64,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(artifact.name, ".._source.bin");
    assert_eq!(artifact.sha256_hex, sha256_hex);
    assert_eq!(artifact.size_bytes, payload.len() as i64);
    assert!(artifact.object_key.starts_with("file-transfer-sources/"));
    assert!(artifact.download_path.ends_with("/artifact"));
    assert_eq!(store.get(&artifact.object_key).await.unwrap(), payload);

    let Json(artifacts) = list_file_transfer_source_artifacts(
        State(state.clone()),
        HeaderMap::new(),
        Query(HistoryQuery { limit: Some(10) }),
    )
    .await
    .unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].id, artifact.id);

    let response =
        download_file_transfer_source_artifact(State(state), HeaderMap::new(), Path(artifact.id))
            .await
            .unwrap();
    assert_eq!(
        response
            .headers()
            .get("x-vpsman-artifact-sha256")
            .unwrap()
            .to_str()
            .unwrap(),
        artifact.sha256_hex
    );
    assert_eq!(
        response
            .headers()
            .get("x-vpsman-artifact-delivery")
            .unwrap()
            .to_str()
            .unwrap(),
        "streamed-filesystem"
    );
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(body.as_ref(), payload.as_slice());
    let _ = tokio::fs::remove_dir_all(store_root).await;
}

#[tokio::test]
async fn file_transfer_source_artifact_upload_rejects_unconfirmed_or_mismatched_payloads() {
    let repo = Repository::Memory(MemoryState::default());
    let store_root = std::env::temp_dir().join(format!(
        "vpsman-transfer-source-reject-store-{}",
        uuid::Uuid::new_v4()
    ));
    let state = test_state_with_store(
        repo,
        BackupObjectStore::filesystem(store_root.clone()).unwrap(),
    );
    let payload = b"source artifact bytes".to_vec();
    let sha256_hex = payload_hash(&payload);

    let unconfirmed = upload_file_transfer_source_artifact(
        State(state.clone()),
        HeaderMap::new(),
        Json(UploadFileTransferSourceArtifactRequest {
            name: None,
            source_base64: BASE64.encode(&payload),
            sha256_hex: sha256_hex.clone(),
            size_bytes: payload.len() as i64,
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        unconfirmed.code,
        "file_transfer_source_confirmation_required"
    );

    let wrong_hash = upload_file_transfer_source_artifact(
        State(state),
        HeaderMap::new(),
        Json(UploadFileTransferSourceArtifactRequest {
            name: None,
            source_base64: BASE64.encode(&payload),
            sha256_hex: "00".repeat(32),
            size_bytes: payload.len() as i64,
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(wrong_hash.code, "file_transfer_source_hash_mismatch");
    let _ = tokio::fs::remove_dir_all(store_root).await;
}

#[test]
fn validates_resumable_file_download_job_documents() {
    let session_id = uuid::Uuid::new_v4();
    let token_hash = payload_hash(b"download-token");

    for command in [
        JobCommand::FileTransferDownloadStart {
            session_id,
            path: "/tmp/download.bin".to_string(),
            chunk_size_bytes: 64 * 1024,
            rate_limit_kbps: 0,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferDownloadChunk {
            session_id,
            offset: 0,
            max_bytes: 64 * 1024,
            resume_token_hash: token_hash.clone(),
        },
    ] {
        validate_job_command(&command).unwrap();
    }
}

#[test]
fn rejects_invalid_resumable_file_download_job_documents() {
    let session_id = uuid::Uuid::new_v4();
    let token_hash = payload_hash(b"download-token");

    for command in [
        JobCommand::FileTransferDownloadStart {
            session_id: uuid::Uuid::nil(),
            path: "/tmp/download.bin".to_string(),
            chunk_size_bytes: 64 * 1024,
            rate_limit_kbps: 0,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferDownloadStart {
            session_id,
            path: "relative.bin".to_string(),
            chunk_size_bytes: 64 * 1024,
            rate_limit_kbps: 0,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferDownloadStart {
            session_id,
            path: "/tmp/download.bin".to_string(),
            chunk_size_bytes: 0,
            rate_limit_kbps: 0,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferDownloadChunk {
            session_id,
            offset: 0,
            max_bytes: 64 * 1024 + 1,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferDownloadChunk {
            session_id,
            offset: 0,
            max_bytes: 64 * 1024,
            resume_token_hash: "not-hex".to_string(),
        },
    ] {
        assert!(validate_job_command(&command).is_err(), "{command:?}");
    }
}
