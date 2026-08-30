use uuid::Uuid;

use crate::{job_request::validate_job_command, model::CreateJobRequest};
use vpsman_common::{
    encode_chunked_file_payload, encode_inline_file_payload, payload_hash, FileActionPolicy,
    FileExistingPolicy, FileOwnershipPolicy, FilePushChunk, JobCommand, MAX_INLINE_FILE_PUSH_BYTES,
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
fn validates_combined_owner_group_file_commands() {
    let data = b"file contents";
    validate_job_command(&JobCommand::FilePush {
        path: "/tmp/vpsman-upload.txt".to_string(),
        mode: 0o640,
        size_bytes: data.len() as u64,
        sha256_hex: payload_hash(data),
        data_base64: encode_inline_file_payload(data).unwrap(),
        existing_policy: FileExistingPolicy::Replace,
        owner: Some("1000:1001".to_string()),
        group: None,
        uid: None,
        gid: None,
        ownership_policy: FileOwnershipPolicy::Fail,
    })
    .unwrap();

    validate_job_command(&JobCommand::FileChown {
        path: "/tmp/vpsman-upload.txt".to_string(),
        owner: Some("operator:ops".to_string()),
        group: None,
        uid: None,
        gid: None,
        recursive: false,
        ownership_policy: FileOwnershipPolicy::Fail,
        policy: FileActionPolicy::Fail,
    })
    .unwrap();
}

#[test]
fn rejects_ambiguous_combined_owner_group_file_command() {
    let command = JobCommand::FileChown {
        path: "/tmp/vpsman-upload.txt".to_string(),
        owner: Some("1000:1001".to_string()),
        group: Some("wheel".to_string()),
        uid: None,
        gid: None,
        recursive: false,
        ownership_policy: FileOwnershipPolicy::Fail,
        policy: FileActionPolicy::Fail,
    };
    assert!(validate_job_command(&command).is_err());

    let command = JobCommand::FileChown {
        path: "/tmp/vpsman-upload.txt".to_string(),
        owner: Some("1000:1001:1002".to_string()),
        group: None,
        uid: None,
        gid: None,
        recursive: false,
        ownership_policy: FileOwnershipPolicy::Fail,
        policy: FileActionPolicy::Fail,
    };
    assert!(validate_job_command(&command).is_err());
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
            follow_symlinks: false,
            policy: FileActionPolicy::Fail,
        },
    ];
    for command in commands {
        assert!(validate_job_command(&command).is_err(), "{command:?}");
    }
}

#[test]
fn file_push_job_command_uses_operation_payload_and_type() {
    let data = b"file contents";
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
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
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
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
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
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
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
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
            follow_symlinks: false,
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
            follow_symlinks: false,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferDownloadStart {
            session_id,
            path: "relative.bin".to_string(),
            chunk_size_bytes: 64 * 1024,
            rate_limit_kbps: 0,
            follow_symlinks: false,
            resume_token_hash: token_hash.clone(),
        },
        JobCommand::FileTransferDownloadStart {
            session_id,
            path: "/tmp/download.bin".to_string(),
            chunk_size_bytes: 0,
            rate_limit_kbps: 0,
            follow_symlinks: false,
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
