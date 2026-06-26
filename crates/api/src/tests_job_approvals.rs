use axum::{extract::State, http::StatusCode, Json};
use tokio::sync::broadcast;
use uuid::Uuid;
use vpsman_common::{AgentCapabilitySnapshot, AgentHello};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{CreateJobApprovalRequest, CreateJobRequest, DecideJobApprovalRequest},
    repository::{MemoryState, Repository},
    repository_ingest::upsert_memory_agent,
    routes_jobs::{approve_job_approval, create_job_approval, reject_job_approval},
    state::AppState,
};

#[tokio::test]
async fn job_approval_approve_dispatches_frozen_request_without_privilege_material() {
    let repo = Repository::Memory(MemoryState::default());
    seed_agent(&repo, "client-a").await;
    let state = test_state(repo.clone());
    let (_operator, headers) = crate::test_auth_context_and_headers(&state).await;
    let job_id = Uuid::new_v4();

    let (status, Json(approval)) = create_job_approval(
        State(state.clone()),
        headers.clone(),
        Json(CreateJobApprovalRequest {
            approval_id: Some(Uuid::new_v4()),
            job: approval_job_request(job_id, "client-a"),
            reason: Some("reviewed destructive package restart".to_string()),
            risk: None,
        }),
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(approval.status, "pending");
    assert_eq!(approval.job_id, job_id);
    assert_eq!(approval.risk, "destructive");
    assert_eq!(approval.target_client_ids, vec!["client-a".to_string()]);

    let (_stored, frozen_request) = repo
        .get_job_approval_request(approval.id)
        .await
        .unwrap()
        .unwrap();
    assert!(frozen_request.privilege_assertion.is_none());
    assert_eq!(frozen_request.max_timeout_secs, Some(60));

    let Json(decision) = approve_job_approval(
        State(state.clone()),
        headers,
        axum::extract::Path(approval.id),
        Json(DecideJobApprovalRequest {
            confirmed: true,
            reason: Some("approved after peer review".to_string()),
        }),
    )
    .await
    .unwrap();

    assert_eq!(decision.approval.status, "approved");
    assert_eq!(
        decision.approval.decision_reason.as_deref(),
        Some("approved after peer review")
    );
    assert_eq!(decision.job.as_ref().map(|job| job.job_id), Some(job_id));
    assert!(repo.get_job(job_id).await.unwrap().is_some());
    let actions = repo
        .list_audit_logs(20)
        .await
        .unwrap()
        .into_iter()
        .map(|audit| audit.action)
        .collect::<Vec<_>>();
    assert!(actions.contains(&"job.approval_requested".to_string()));
    assert!(actions.contains(&"job.approval_approved".to_string()));
    assert!(actions.contains(&"job.dispatch_requested".to_string()));
}

#[tokio::test]
async fn job_approval_reject_records_decision_without_dispatching_job() {
    let repo = Repository::Memory(MemoryState::default());
    seed_agent(&repo, "client-a").await;
    let state = test_state(repo.clone());
    let (_operator, headers) = crate::test_auth_context_and_headers(&state).await;
    let job_id = Uuid::new_v4();
    let (_status, Json(approval)) = create_job_approval(
        State(state.clone()),
        headers.clone(),
        Json(CreateJobApprovalRequest {
            approval_id: None,
            job: approval_job_request(job_id, "client-a"),
            reason: None,
            risk: Some("maintenance".to_string()),
        }),
    )
    .await
    .unwrap();

    let Json(decision) = reject_job_approval(
        State(state.clone()),
        headers,
        axum::extract::Path(approval.id),
        Json(DecideJobApprovalRequest {
            confirmed: true,
            reason: Some("window closed".to_string()),
        }),
    )
    .await
    .unwrap();

    assert_eq!(decision.approval.status, "rejected");
    assert_eq!(decision.approval.risk, "maintenance");
    assert!(decision.job.is_none());
    assert!(repo.get_job(job_id).await.unwrap().is_none());
}

fn approval_job_request(job_id: Uuid, client_id: &str) -> CreateJobRequest {
    CreateJobRequest {
        job_id: Some(job_id),
        selector_expression: format!("id:{client_id}"),
        target_client_ids: vec![client_id.to_string()],
        destructive: true,
        confirmed: true,
        command: "systemctl restart app".to_string(),
        argv: vec![
            "systemctl".to_string(),
            "restart".to_string(),
            "app".to_string(),
        ],
        operation: None,
        max_timeout_secs: Some(60),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    }
}

async fn seed_agent(repo: &Repository, client_id: &str) {
    if let Repository::Memory(memory) = repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: client_id.to_string(),
                process_incarnation_id: Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot::default(),
            },
        )
        .await;
    }
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: Some("test-internal-token".to_string()),
        gateway: GatewayDispatchClient::new(
            Some("http://127.0.0.1:9".to_string()),
            Some("test-internal-token".to_string()),
        )
        .with_test_privilege_auto_approve(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}
