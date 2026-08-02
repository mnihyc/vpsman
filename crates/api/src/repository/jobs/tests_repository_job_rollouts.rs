use vpsman_common::JobCommand;

use super::*;
use crate::{
    repository::MemoryState,
    tests::{operation_job_request, seed_never_connected_memory_agent, test_operator},
    TargetDispatchOutcome,
};

fn terminal_outcome(status: &str) -> TargetDispatchOutcome {
    TargetDispatchOutcome {
        status: status.to_string(),
        exit_code: (status == "completed").then_some(0),
        command_version: Some(1),
        accepted: true,
        message: status.to_string(),
        received_at: None,
        outputs: Vec::new(),
    }
}

fn rollout_request(targets: &[&str]) -> crate::model::CreateJobRequest {
    let mut request = operation_job_request(
        JobCommand::AgentUpdateCheck {
            version_url: None,
            activate: false,
            restart_agent: false,
        },
        targets,
    );
    request.rollout = Some(crate::model::JobRolloutPolicy {
        canary_client_ids: vec![targets[0].to_string()],
        batch_size: 2,
        max_failures: 0,
        pause_after_canary: true,
        batch_delay_secs: 0,
    });
    request
}

#[tokio::test]
async fn rollout_releases_only_reviewed_batches_and_survives_failure_pause() {
    let repo = Repository::Memory(MemoryState::default());
    for client_id in ["client-a", "client-b", "client-c", "client-d"] {
        seed_never_connected_memory_agent(&repo, client_id).await;
    }
    let operator = test_operator();
    let request = rollout_request(&["client-a", "client-b", "client-c", "client-d"]);
    let job_id = Uuid::new_v4();
    repo.record_dispatching_job(
        job_id,
        &request,
        "command-hash",
        "request-fingerprint",
        &operator,
        &request.target_client_ids,
    )
    .await
    .unwrap();

    let canary = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
    assert_eq!(
        canary
            .iter()
            .map(|target| target.client_id.as_str())
            .collect::<Vec<_>>(),
        vec!["client-a"]
    );
    repo.update_job_target_result(job_id, "client-a", &terminal_outcome("completed"))
        .await
        .unwrap();
    assert_eq!(repo.reconcile_job_rollouts(10).await.unwrap(), 1);
    let paused = repo.get_job_rollout(job_id).await.unwrap().unwrap();
    assert_eq!(paused.status, "paused");
    assert_eq!(paused.pause_reason.as_deref(), Some("canary_review"));
    assert_eq!(paused.current_batch, 1);
    assert!(repo
        .claim_due_job_targets(10, 30, 0)
        .await
        .unwrap()
        .is_empty());

    repo.resume_job_rollout(job_id, &operator, Some("canary accepted"))
        .await
        .unwrap();
    let first_batch = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
    assert_eq!(
        first_batch
            .iter()
            .map(|target| target.client_id.as_str())
            .collect::<Vec<_>>(),
        vec!["client-b", "client-c"]
    );
    repo.update_job_target_result(job_id, "client-b", &terminal_outcome("failed"))
        .await
        .unwrap();
    repo.update_job_target_result(job_id, "client-c", &terminal_outcome("completed"))
        .await
        .unwrap();
    assert_eq!(repo.reconcile_job_rollouts(10).await.unwrap(), 1);
    let failure_pause = repo.get_job_rollout(job_id).await.unwrap().unwrap();
    assert_eq!(failure_pause.status, "paused");
    assert_eq!(
        failure_pause.pause_reason.as_deref(),
        Some("failure_threshold")
    );
    assert_eq!(failure_pause.current_batch, 2);
    assert!(repo
        .claim_due_job_targets(10, 30, 0)
        .await
        .unwrap()
        .is_empty());

    repo.resume_job_rollout(job_id, &operator, Some("failure reviewed"))
        .await
        .unwrap();
    let final_batch = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
    assert_eq!(final_batch.len(), 1);
    assert_eq!(final_batch[0].client_id, "client-d");
    repo.update_job_target_result(job_id, "client-d", &terminal_outcome("completed"))
        .await
        .unwrap();
    assert_eq!(repo.reconcile_job_rollouts(10).await.unwrap(), 1);
    let completed = repo.get_job_rollout(job_id).await.unwrap().unwrap();
    assert_eq!(completed.status, "completed");
    assert!(completed.completed_at.is_some());
    assert_eq!(completed.targets.len(), 4);
}

#[tokio::test]
async fn malformed_current_batch_is_paused_without_starving_healthy_rollout() {
    let repo = Repository::Memory(MemoryState::default());
    for client_id in [
        "broken-a",
        "broken-b",
        "broken-c",
        "broken-d",
        "healthy-a",
        "healthy-b",
        "healthy-c",
    ] {
        seed_never_connected_memory_agent(&repo, client_id).await;
    }
    let operator = test_operator();
    let malformed_job_id = Uuid::new_v4();
    let mut malformed_request = rollout_request(&["broken-a", "broken-b", "broken-c", "broken-d"]);
    malformed_request
        .rollout
        .as_mut()
        .unwrap()
        .pause_after_canary = false;
    repo.record_dispatching_job(
        malformed_job_id,
        &malformed_request,
        "malformed-command-hash",
        "malformed-request-fingerprint",
        &operator,
        &malformed_request.target_client_ids,
    )
    .await
    .unwrap();

    let healthy_job_id = Uuid::new_v4();
    let mut healthy_request = rollout_request(&["healthy-a", "healthy-b", "healthy-c"]);
    healthy_request.rollout.as_mut().unwrap().pause_after_canary = false;
    repo.record_dispatching_job(
        healthy_job_id,
        &healthy_request,
        "healthy-command-hash",
        "healthy-request-fingerprint",
        &operator,
        &healthy_request.target_client_ids,
    )
    .await
    .unwrap();

    if let Repository::Memory(memory) = &repo {
        memory
            .job_rollouts
            .write()
            .await
            .iter_mut()
            .find(|rollout| rollout.job_id == malformed_job_id)
            .unwrap()
            .current_batch = 1;
        memory
            .job_rollout_targets
            .write()
            .await
            .retain(|(job_id, _), batch| *job_id != malformed_job_id || *batch != 1);
        let now = unix_now().to_string();
        let mut targets = memory.job_targets.write().await;
        let healthy_canary = targets
            .iter_mut()
            .find(|target| target.job_id == healthy_job_id && target.client_id == "healthy-a")
            .unwrap();
        healthy_canary.status = "completed".to_string();
        healthy_canary.completed_at = Some(now);
    }

    assert_eq!(repo.reconcile_job_rollouts(1).await.unwrap(), 1);
    let malformed = repo
        .get_job_rollout(malformed_job_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(malformed.status, ROLLOUT_STATUS_PAUSED);
    assert_eq!(
        malformed.pause_reason.as_deref(),
        Some(ROLLOUT_PAUSE_CURRENT_BATCH_ASSIGNMENT_MISSING)
    );
    assert_eq!(
        repo.get_job_rollout(healthy_job_id)
            .await
            .unwrap()
            .unwrap()
            .current_batch,
        0
    );

    assert_eq!(repo.reconcile_job_rollouts(1).await.unwrap(), 1);
    let healthy = repo.get_job_rollout(healthy_job_id).await.unwrap().unwrap();
    assert_eq!(healthy.status, ROLLOUT_STATUS_RUNNING);
    assert_eq!(healthy.current_batch, 1);
}

#[tokio::test]
async fn cancel_aborts_rollout_and_prevents_unreleased_claims() {
    let repo = Repository::Memory(MemoryState::default());
    for client_id in ["client-a", "client-b", "client-c"] {
        seed_never_connected_memory_agent(&repo, client_id).await;
    }
    let operator = test_operator();
    let request = rollout_request(&["client-a", "client-b", "client-c"]);
    let job_id = Uuid::new_v4();
    repo.record_dispatching_job(
        job_id,
        &request,
        "command-hash",
        "request-fingerprint",
        &operator,
        &request.target_client_ids,
    )
    .await
    .unwrap();
    assert_eq!(
        repo.claim_due_job_targets(10, 30, 0).await.unwrap().len(),
        1
    );

    let cancel = repo
        .request_job_cancel(job_id, &operator, Some("stop rollout"))
        .await
        .unwrap();
    assert_eq!(cancel.pending_canceled, 2);
    assert_eq!(cancel.cancel_targets, vec!["client-a"]);
    let rollout = repo.get_job_rollout(job_id).await.unwrap().unwrap();
    assert_eq!(rollout.status, "aborted");
    assert_eq!(rollout.pause_reason.as_deref(), Some("stop rollout"));
    assert!(repo
        .claim_due_job_targets(10, 30, 0)
        .await
        .unwrap()
        .is_empty());
}
