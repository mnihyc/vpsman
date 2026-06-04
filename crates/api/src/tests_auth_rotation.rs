use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::{
    model::{JobHistoryView, JobTargetView},
    repository::{MemoryState, Repository},
};

#[tokio::test]
async fn auth_proof_rotation_history_is_sanitized_and_counted() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        memory.jobs.write().await.push(JobHistoryView {
            id: job_id,
            actor_id: None,
            command_type: "auth_proof_key_rotate".to_string(),
            privileged: true,
            status: "completed".to_string(),
            target_count: 3,
            payload_hash: "ab".repeat(32),
            created_at: "1700000000".to_string(),
            completed_at: Some("1700000005".to_string()),
        });
        memory.job_operations.write().await.insert(
            job_id,
            JobCommand::AuthProofKeyRotate {
                new_proof_key_hex: "11".repeat(32),
                rotation_generation: Some("2026-q2".to_string()),
            },
        );
        memory.job_targets.write().await.extend([
            JobTargetView {
                job_id,
                client_id: "edge-a".to_string(),
                status: "completed".to_string(),
                exit_code: Some(0),
                started_at: Some("1700000001".to_string()),
                completed_at: Some("1700000002".to_string()),
            },
            JobTargetView {
                job_id,
                client_id: "edge-b".to_string(),
                status: "rejected_by_agent".to_string(),
                exit_code: None,
                started_at: Some("1700000001".to_string()),
                completed_at: Some("1700000002".to_string()),
            },
            JobTargetView {
                job_id,
                client_id: "edge-c".to_string(),
                status: "queued".to_string(),
                exit_code: None,
                started_at: None,
                completed_at: None,
            },
        ]);
    }

    let rows = repo.list_auth_proof_rotation_history(10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rotation_generation.as_deref(), Some("2026-q2"));
    assert_eq!(rows[0].completed_count, 1);
    assert_eq!(rows[0].failed_count, 1);
    assert_eq!(rows[0].pending_count, 1);

    let serialized = serde_json::to_string(&rows).unwrap();
    assert!(!serialized.contains("new_proof_key_hex"));
    assert!(!serialized.contains(&"11".repeat(32)));
}
