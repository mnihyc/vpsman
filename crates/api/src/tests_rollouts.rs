use axum::{
    extract::{Path, Query, State},
    Json,
};
use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::{
    model::{JobRolloutPolicy, UpdateJobRolloutRequest},
    repository::{MemoryState, Repository},
    routes_job_rollouts::{
        get_job_rollout, list_job_rollouts, pause_job_rollout, resume_job_rollout,
        JobRolloutListQuery,
    },
    tests::{operation_job_request, test_app_state, test_operator},
};

async fn recorded_rollout() -> (crate::state::AppState, Uuid) {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let mut request = operation_job_request(
        JobCommand::AgentUpdateCheck {
            version_url: None,
            activate: false,
            restart_agent: false,
        },
        &["client-a", "client-b", "client-c"],
    );
    request.rollout = Some(JobRolloutPolicy {
        canary_client_ids: vec!["client-a".to_string()],
        batch_size: 1,
        max_failures: 0,
        pause_after_canary: true,
        batch_delay_secs: 15,
    });
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
    (test_app_state(repo), job_id)
}

#[tokio::test]
async fn rollout_routes_keep_pause_and_resume_explicit() {
    let (state, job_id) = recorded_rollout().await;
    let headers = crate::test_auth_headers(&state).await;

    let listed = list_job_rollouts(
        State(state.clone()),
        headers.clone(),
        Query(JobRolloutListQuery { limit: Some(10) }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].job_id, job_id);

    let paused = pause_job_rollout(
        State(state.clone()),
        headers.clone(),
        Path(job_id),
        Json(UpdateJobRolloutRequest {
            confirmed: false,
            reason: Some("operator review".to_string()),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(paused.status, "paused");
    assert_eq!(paused.pause_reason.as_deref(), Some("operator review"));

    let error = resume_job_rollout(
        State(state.clone()),
        headers.clone(),
        Path(job_id),
        Json(UpdateJobRolloutRequest {
            confirmed: false,
            reason: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "job_rollout_resume_requires_confirmation");

    let resumed = resume_job_rollout(
        State(state.clone()),
        headers.clone(),
        Path(job_id),
        Json(UpdateJobRolloutRequest {
            confirmed: true,
            reason: Some("stage reviewed".to_string()),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(resumed.status, "running");
    assert!(resumed.pause_reason.is_none());

    let loaded = get_job_rollout(State(state), headers, Path(job_id))
        .await
        .unwrap()
        .0;
    assert_eq!(loaded.job_id, job_id);
    assert_eq!(loaded.targets.len(), 3);
}
