use super::{
    append_process_supervisor_inventory, build_process_supervisor_inventory,
    ensure_process_supervisor_inventory_complete, JobOutputPersistConfig, JobOutputWriteResult,
    SupervisorInventoryOutput, PROCESS_SUPERVISOR_INVENTORY_PAGE_SIZE,
    PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR,
};
use crate::{object_store::BackupObjectStore, repository::MemoryState, Repository};
use base64::Engine as _;
use std::collections::BTreeSet;
use uuid::Uuid;
use vpsman_common::{payload_hash, CommandOutput, OutputStream};

#[tokio::test]
async fn externalizes_large_non_status_outputs_to_object_store() {
    let repo = Repository::Memory(MemoryState::default());
    let root = std::env::temp_dir().join(format!("vpsman-job-output-store-{}", Uuid::new_v4()));
    let store = BackupObjectStore::filesystem(root.clone()).unwrap();
    let job_id = Uuid::new_v4();
    let data = b"large retained output".repeat(8);

    repo.record_job_outputs_with_config(
        job_id,
        "client-a",
        &[
            CommandOutput {
                job_id,
                stream: OutputStream::Stdout,
                data: data.clone(),
                exit_code: None,
                done: false,
            },
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: br#"{"type":"ok"}"#.to_vec(),
                exit_code: Some(0),
                done: true,
            },
        ],
        JobOutputPersistConfig {
            object_store: Some(&store),
            artifact_min_bytes: 16,
        },
    )
    .await
    .unwrap();

    let outputs = repo.list_job_outputs(job_id).await.unwrap();
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].storage, "object_store");
    assert_eq!(outputs[0].data_base64, super::BASE64.encode(&data));
    let expected_hash = payload_hash(&data);
    assert_eq!(
        outputs[0].artifact_sha256_hex.as_deref(),
        Some(expected_hash.as_str())
    );
    let artifact = repo
        .get_job_output_artifact_ref(job_id, "client-a", 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(store.get(&artifact.object_key).await.unwrap(), data);
    assert_eq!(outputs[1].storage, "inline");
    assert!(!outputs[1].data_base64.is_empty());
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn rejects_oversized_status_output() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: vec![b'x'; vpsman_server_core::STATUS_OUTPUT_MAX_BYTES + 1],
        exit_code: Some(1),
        done: true,
    };

    let error = repo
        .record_job_outputs_with_config(
            job_id,
            "client-a",
            &[output],
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("status output exceeds max bytes"));
}

#[tokio::test]
async fn incremental_output_recording_is_idempotent_by_sequence() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    let first = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"hello".to_vec(),
        exit_code: None,
        done: false,
    };
    let done = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"ok"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };

    repo.record_job_output_chunk_with_config(
        job_id,
        "client-a",
        0,
        &first,
        None,
        JobOutputPersistConfig {
            object_store: None,
            artifact_min_bytes: usize::MAX,
        },
    )
    .await
    .unwrap();
    repo.record_job_outputs_with_config(
        job_id,
        "client-a",
        &[first, done],
        JobOutputPersistConfig {
            object_store: None,
            artifact_min_bytes: usize::MAX,
        },
    )
    .await
    .unwrap();

    let outputs = repo.list_job_outputs(job_id).await.unwrap();
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].seq, 0);
    assert_eq!(outputs[1].seq, 1);
    assert!(outputs[1].done);
}

#[tokio::test]
async fn duplicate_conflicting_sequence_reports_conflict_without_replacing_output() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    let first = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"first".to_vec(),
        exit_code: None,
        done: false,
    };
    let final_conflict = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"completed"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };

    let inserted = repo
        .record_job_output_chunk_checked_with_config(
            job_id,
            "client-a",
            0,
            &first,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();
    let duplicate = repo
        .record_job_output_chunk_checked_with_config(
            job_id,
            "client-a",
            0,
            &first,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();
    let conflict = repo
        .record_job_output_chunk_checked_with_config(
            job_id,
            "client-a",
            0,
            &final_conflict,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();

    assert_eq!(inserted, JobOutputWriteResult::Inserted);
    assert_eq!(duplicate, JobOutputWriteResult::DuplicateIdentical);
    assert_eq!(conflict, JobOutputWriteResult::DuplicateConflict);
    let outputs = repo.list_job_outputs(job_id).await.unwrap();
    assert_eq!(outputs.len(), 1);
    assert!(!outputs[0].done);
    assert_eq!(outputs[0].data_base64, super::BASE64.encode(b"first"));
    let audits = repo.list_audit_logs(10).await.unwrap();
    assert!(audits
        .iter()
        .any(|audit| audit.action == "job.output_conflict_ignored"));
}

#[tokio::test]
async fn batch_conflict_poisons_later_final_output_insert() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    let first = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"first".to_vec(),
        exit_code: None,
        done: false,
    };
    repo.record_job_output_chunk_with_config(
        job_id,
        "client-a",
        0,
        &first,
        None,
        JobOutputPersistConfig {
            object_store: None,
            artifact_min_bytes: usize::MAX,
        },
    )
    .await
    .unwrap();

    let conflicting_replay = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"different"}"#.to_vec(),
        exit_code: Some(1),
        done: false,
    };
    let later_final = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"completed"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };
    let results = repo
        .record_job_outputs_checked_with_config(
            job_id,
            "client-a",
            &[conflicting_replay, later_final],
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();

    assert!(results.contains(&JobOutputWriteResult::DuplicateConflict));
    let outputs = repo.list_job_outputs(job_id).await.unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].seq, 0);
    assert!(!outputs[0].done);
    assert_eq!(outputs[0].data_base64, super::BASE64.encode(b"first"));
}

#[tokio::test]
async fn conflicting_replay_output_preserves_original_sequence_row() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    let original = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"original output".to_vec(),
        exit_code: None,
        done: false,
    };
    let replay_conflict = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"duplicate_job_replayed"}"#.to_vec(),
        exit_code: Some(75),
        done: true,
    };

    repo.record_job_output_chunk_with_config(
        job_id,
        "client-a",
        0,
        &original,
        None,
        JobOutputPersistConfig {
            object_store: None,
            artifact_min_bytes: usize::MAX,
        },
    )
    .await
    .unwrap();
    repo.record_job_output_chunk_with_config(
        job_id,
        "client-a",
        0,
        &replay_conflict,
        None,
        JobOutputPersistConfig {
            object_store: None,
            artifact_min_bytes: usize::MAX,
        },
    )
    .await
    .unwrap();

    let outputs = repo.list_job_outputs(job_id).await.unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].stream, "stdout");
    assert_eq!(
        outputs[0].data_base64,
        super::BASE64.encode(b"original output")
    );
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    assert!(memory
        .audits
        .read()
        .await
        .iter()
        .any(|audit| audit.action == "job.output_conflict_ignored"
            && audit.metadata["seq"].as_i64() == Some(0)));
}

#[test]
fn builds_deduplicated_supervisor_inventory_from_latest_outputs() {
    let start_job = Uuid::new_v4();
    let status_job = Uuid::new_v4();
    let outputs = vec![
        SupervisorInventoryOutput {
            job_id: status_job,
            client_id: "edge-a".to_string(),
            stream: "stdout".to_string(),
            data: serde_json::to_vec(&serde_json::json!({
                "type": "process_status",
                "processes": [{
                    "name": "ospf-worker",
                    "status": "running",
                    "pid": 4242,
                    "started_unix": 1700000000_u64,
                    "stdout_log": "/tmp/ospf.stdout.log",
                    "stderr_log": "/tmp/ospf.stderr.log",
                    "restart_attempts": 2,
                    "last_exit_code": 7,
                    "last_exit_unix": 1700000010_u64,
                    "last_restart_unix": 1700000011_u64,
                    "limit_effectiveness": {
                        "overall": { "status": "degraded_desired_only" }
                    },
                    "cgroup_status": {
                        "status": "available",
                        "process_count": 2,
                        "cpu_weight": 39,
                        "memory_current_bytes": 1048576,
                        "pids_current": 2
                    }
                }]
            }))
            .unwrap(),
            created_at: "200".to_string(),
            command_type: "process_status".to_string(),
        },
        SupervisorInventoryOutput {
            job_id: start_job,
            client_id: "edge-a".to_string(),
            stream: "status".to_string(),
            data: serde_json::to_vec(&serde_json::json!({
                "type": "process_start",
                "name": "ospf-worker",
                "status": "running",
                "pid": 4000
            }))
            .unwrap(),
            created_at: "100".to_string(),
            command_type: "process_start".to_string(),
        },
    ];

    let inventory = build_process_supervisor_inventory(outputs, 50);

    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].client_id, "edge-a");
    assert_eq!(inventory[0].name, "ospf-worker");
    assert_eq!(inventory[0].pid, Some(4242));
    assert_eq!(inventory[0].source_job_id, status_job);
    assert_eq!(inventory[0].source_command_type, "process_status");
    assert_eq!(inventory[0].restart_attempts, Some(2));
    assert_eq!(inventory[0].last_exit_code, Some(7));
    assert_eq!(inventory[0].last_exit_unix, Some(1700000010));
    assert_eq!(inventory[0].last_restart_unix, Some(1700000011));
    assert_eq!(
        inventory[0].limit_effectiveness_status.as_deref(),
        Some("degraded_desired_only")
    );
    assert_eq!(inventory[0].cgroup_status.as_deref(), Some("available"));
    assert_eq!(inventory[0].cgroup_process_count, Some(2));
    assert_eq!(inventory[0].cgroup_cpu_weight, Some(39));
    assert_eq!(inventory[0].cgroup_memory_current_bytes, Some(1048576));
    assert_eq!(inventory[0].cgroup_pids_current, Some(2));
}

#[test]
fn ignores_non_inventory_output_shapes() {
    let inventory = build_process_supervisor_inventory(
        vec![SupervisorInventoryOutput {
            job_id: Uuid::new_v4(),
            client_id: "edge-a".to_string(),
            stream: "stdout".to_string(),
            data: b"not json".to_vec(),
            created_at: "100".to_string(),
            command_type: "process_status".to_string(),
        }],
        50,
    );

    assert!(inventory.is_empty());
}

#[test]
fn supervisor_inventory_continues_past_repeated_output_pages() {
    let process_output = |name: &str, created_at: usize| SupervisorInventoryOutput {
        job_id: Uuid::new_v4(),
        client_id: "edge-a".to_string(),
        stream: "stdout".to_string(),
        data: serde_json::to_vec(&serde_json::json!({
            "type": "process_status",
            "processes": [{ "name": name, "status": "running" }]
        }))
        .unwrap(),
        created_at: format!("{created_at:05}"),
        command_type: "process_status".to_string(),
    };
    let mut outputs = (1..=5_001)
        .rev()
        .map(|created_at| process_output("frequent", created_at))
        .collect::<Vec<_>>();
    outputs.push(process_output("quiet", 0));
    let mut outputs = outputs.into_iter();
    let mut seen = BTreeSet::new();
    let mut inventory = Vec::new();
    loop {
        let page = outputs
            .by_ref()
            .take(PROCESS_SUPERVISOR_INVENTORY_PAGE_SIZE as usize)
            .collect::<Vec<_>>();
        if page.is_empty() {
            break;
        }
        if append_process_supervisor_inventory(page, &mut seen, &mut inventory, 2) {
            break;
        }
    }

    assert_eq!(inventory.len(), 2);
    assert_eq!(inventory[0].name, "frequent");
    assert_eq!(inventory[1].name, "quiet");
}

#[test]
fn supervisor_inventory_never_returns_an_incomplete_bounded_scan() {
    ensure_process_supervisor_inventory_complete(2, 2, false).unwrap();
    ensure_process_supervisor_inventory_complete(1, 2, true).unwrap();
    assert_eq!(
        ensure_process_supervisor_inventory_complete(1, 2, false)
            .unwrap_err()
            .to_string(),
        PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR
    );
}
