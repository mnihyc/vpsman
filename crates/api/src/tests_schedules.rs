use super::*;
use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use vpsman_common::{
    derive_super_key, encode_json, payload_hash, plan_tunnel, random_nonce, sign_privilege_proof,
    AgentCapabilitySnapshot, AgentPrivilegeMode, BandwidthTier, CommandEnvelope, CommandOutput,
    GatewayCommandDispatch, GatewayCommandDispatchResult, JobCommand, OspfCostPolicy, OutputStream,
    TunnelEndpointSide, TunnelKind, TunnelPlanInput,
};

use crate::routes_jobs::{cancel_job, dispatch_scheduled_job, CancelJobRequest};

const TEST_INTERNAL_TOKEN: &str = "test-internal-token-value-32-plus-chars";

#[tokio::test]
async fn schedule_create_lists_durable_selector_without_plaintext_proof_material() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let request = CreateScheduleRequest {
        name: "nightly-uptime".to_string(),
        operation: JobCommand::Shell {
            argv: vec!["/usr/bin/uptime".to_string()],
            pty: false,
        },
        clients: Vec::new(),
        pools: Vec::new(),
        tags: vec!["edge".to_string()],
        interval_secs: 3600,
        start_at_unix: Some(unix_now()),
        enabled: true,
        catch_up_policy: "run_all_limited".to_string(),
        catch_up_limit: 3,
        retry_delay_secs: 120,
        max_failures: 5,
    };

    validate_schedule_request(&request).unwrap();
    let schedule = repo.create_schedule(request, &operator).await.unwrap();
    let schedules = repo.list_schedules().await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(schedule.name, "nightly-uptime");
    assert_eq!(schedule.command_type, "shell_argv");
    assert_eq!(schedule.tags, vec!["edge"]);
    assert_eq!(schedule.catch_up_policy, "run_all_limited");
    assert_eq!(schedule.catch_up_limit, 3);
    assert_eq!(schedule.retry_delay_secs, 120);
    assert_eq!(schedule.max_failures, 5);
    assert_eq!(schedule.failure_count, 0);
    assert_eq!(schedule.last_error, None);
    assert_eq!(schedules.len(), 1);
    assert_eq!(audits[0].action, "schedule.upserted");
    assert!(!serde_json::to_string(&audits[0].metadata)
        .unwrap()
        .contains("correct horse"));
}

#[test]
fn schedule_validation_rejects_unsafe_or_empty_requests() {
    let mut request = CreateScheduleRequest {
        name: "bad".to_string(),
        operation: JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        },
        clients: Vec::new(),
        pools: Vec::new(),
        tags: Vec::new(),
        interval_secs: 60,
        start_at_unix: None,
        enabled: true,
        catch_up_policy: "skip_missed".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 300,
        max_failures: 3,
    };

    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.tags.push("edge".to_string());
    request.interval_secs = 0;
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.interval_secs = 60;
    request.operation = JobCommand::Shell {
        argv: vec!["/bin/sh".to_string()],
        pty: true,
    };
    assert!(validate_schedule_request(&request).is_ok());
    request.operation = JobCommand::Shell {
        argv: Vec::new(),
        pty: false,
    };
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.operation = JobCommand::Shell {
        argv: vec!["/bin/true".to_string()],
        pty: false,
    };
    request.catch_up_policy = "retry_everything".to_string();
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.catch_up_policy = "skip_missed".to_string();
    request.catch_up_limit = 0;
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.catch_up_limit = 1;
    request.retry_delay_secs = 0;
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.retry_delay_secs = 300;
    request.max_failures = 0;
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn scheduled_job_dispatch_requires_fresh_proof_and_uses_frozen_targets() {
    let repo = Repository::Memory(MemoryState::default());
    let memory = match &repo {
        Repository::Memory(memory) => memory.clone(),
        Repository::Postgres(_) => unreachable!("test uses memory repository"),
    };
    let job_id = Uuid::new_v4();
    let operation = JobCommand::Shell {
        argv: vec!["/usr/bin/true".to_string()],
        pty: false,
    };
    let payload = encode_json(&operation).unwrap();
    let payload_hash = payload_hash(&payload);
    memory.jobs.write().await.push(JobHistoryView {
        id: job_id,
        actor_id: Some(Uuid::nil()),
        command_type: "scheduled_shell_argv".to_string(),
        privileged: true,
        status: "approval_required".to_string(),
        target_count: 1,
        payload_hash: payload_hash.clone(),
        created_at: unix_now().to_string(),
        completed_at: None,
    });
    memory.job_targets.write().await.push(JobTargetView {
        job_id,
        client_id: "client-a".to_string(),
        status: "approval_required".to_string(),
        exit_code: None,
        started_at: None,
        completed_at: None,
    });
    memory.scheduled_jobs.write().await.insert(
        job_id,
        ScheduledJobDispatchRecord {
            job_id,
            source_schedule_id: None,
            actor_id: Some(Uuid::nil()),
            command_type: "scheduled_shell_argv".to_string(),
            operation: operation.clone(),
            payload_hash: payload_hash.clone(),
            targets: vec!["client-a".to_string()],
        },
    );

    let (gateway_url, gateway_task) = spawn_fake_gateway_once().await;
    let signing_key = SigningKey::from_bytes(&[21_u8; 32]);
    let state = AppState {
        repo: repo.clone(),
        events: broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::new(
            Some(gateway_url),
            Some(TEST_INTERNAL_TOKEN.to_string()),
        ),
        server_signing_key: Some(Arc::new(signing_key)),
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    };
    let envelope = proof_envelope(job_id, "client-a", &payload_hash);
    let request = DispatchScheduledJobRequest {
        confirmed: true,
        timeout_secs: Some(5),
        force_unprivileged: false,
        envelope: Some(envelope),
        envelopes: HashMap::new(),
    };

    let (status, Json(response)) =
        dispatch_scheduled_job(State(state), HeaderMap::new(), Path(job_id), Json(request))
            .await
            .unwrap();
    let dispatch = gateway_task.await.unwrap();
    let jobs = repo.list_jobs(10).await.unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();
    let outputs = repo.list_job_outputs(job_id).await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.job_id, job_id);
    assert_eq!(response.accepted_targets, 1);
    assert_eq!(response.status, "completed");
    assert_eq!(dispatch.client_id, "client-a");
    assert_eq!(dispatch.request.job_id, job_id);
    assert_eq!(encode_json(&dispatch.request.command).unwrap(), payload);
    assert!(!dispatch.request.envelope.server_signature.is_empty());
    assert_eq!(jobs[0].status, "completed");
    assert_eq!(targets[0].status, "completed");
    assert_eq!(outputs.len(), 1);
    assert_eq!(audits[0].action, "job.target_result");
    assert!(audits
        .iter()
        .any(|audit| audit.action == "schedule.dispatch_approved"));
}

#[tokio::test]
async fn scheduled_network_dispatch_degrades_unprivileged_target_without_gateway() {
    let repo = Repository::Memory(MemoryState::default());
    let memory = match &repo {
        Repository::Memory(memory) => memory.clone(),
        Repository::Postgres(_) => unreachable!("test uses memory repository"),
    };
    let job_id = Uuid::new_v4();
    let operation = scheduled_network_rollback_command();
    let payload = encode_json(&operation).unwrap();
    let payload_hash = payload_hash(&payload);
    memory.agents.write().await.push(AgentView {
        id: "client-a".to_string(),
        display_name: "client-a".to_string(),
        status: "connected".to_string(),
        tags: Vec::new(),
        capabilities: AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            effective_uid: Some(1000),
            can_attempt_privileged_ops: true,
            can_manage_runtime_tunnels: false,
            can_apply_process_limits: false,
            command_protocol_version: 1,
            unprivileged_hint: Some("running without root".to_string()),
        },
    });
    memory.jobs.write().await.push(JobHistoryView {
        id: job_id,
        actor_id: Some(Uuid::nil()),
        command_type: "scheduled_network_rollback".to_string(),
        privileged: true,
        status: "approval_required".to_string(),
        target_count: 1,
        payload_hash: payload_hash.clone(),
        created_at: unix_now().to_string(),
        completed_at: None,
    });
    memory.job_targets.write().await.push(JobTargetView {
        job_id,
        client_id: "client-a".to_string(),
        status: "approval_required".to_string(),
        exit_code: None,
        started_at: None,
        completed_at: None,
    });
    memory.scheduled_jobs.write().await.insert(
        job_id,
        ScheduledJobDispatchRecord {
            job_id,
            source_schedule_id: None,
            actor_id: Some(Uuid::nil()),
            command_type: "scheduled_network_rollback".to_string(),
            operation,
            payload_hash: payload_hash.clone(),
            targets: vec!["client-a".to_string()],
        },
    );
    let signing_key = SigningKey::from_bytes(&[21_u8; 32]);
    let state = AppState {
        repo: repo.clone(),
        events: broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: Some(Arc::new(signing_key)),
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    };
    let request = DispatchScheduledJobRequest {
        confirmed: true,
        timeout_secs: Some(5),
        force_unprivileged: false,
        envelope: Some(proof_envelope(job_id, "client-a", &payload_hash)),
        envelopes: HashMap::new(),
    };

    let (status, Json(response)) =
        dispatch_scheduled_job(State(state), HeaderMap::new(), Path(job_id), Json(request))
            .await
            .unwrap();
    let job = repo.get_job(job_id).await.unwrap().unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();
    let outputs = repo.list_job_outputs(job_id).await.unwrap();
    let output_bytes = BASE64_STANDARD.decode(&outputs[0].data_base64).unwrap();
    let status_output: serde_json::Value = serde_json::from_slice(&output_bytes).unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.accepted_targets, 0);
    assert_eq!(response.status, "degraded_unprivileged");
    assert_eq!(job.status, "degraded_unprivileged");
    assert_eq!(targets[0].status, "degraded_unprivileged");
    assert_eq!(
        status_output["reason"],
        "target_agent_lacks_root_runtime_network_capability"
    );
}

#[tokio::test]
async fn scheduled_job_dispatch_rejects_missing_confirmation() {
    let repo = Repository::Memory(MemoryState::default());
    let state = AppState {
        repo,
        events: broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    };

    let error = dispatch_scheduled_job(
        State(state),
        HeaderMap::new(),
        Path(Uuid::new_v4()),
        Json(DispatchScheduledJobRequest {
            confirmed: false,
            timeout_secs: None,
            force_unprivileged: false,
            envelope: None,
            envelopes: HashMap::new(),
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "scheduled_dispatch_confirmation_required");
}

#[tokio::test]
async fn scheduled_approval_job_can_be_canceled_before_dispatch() {
    let repo = Repository::Memory(MemoryState::default());
    let memory = match &repo {
        Repository::Memory(memory) => memory.clone(),
        Repository::Postgres(_) => unreachable!("test uses memory repository"),
    };
    let job_id = Uuid::new_v4();
    let operation = JobCommand::Shell {
        argv: vec!["/usr/bin/true".to_string()],
        pty: false,
    };
    let payload_hash = payload_hash(&encode_json(&operation).unwrap());
    memory.jobs.write().await.push(JobHistoryView {
        id: job_id,
        actor_id: Some(Uuid::nil()),
        command_type: "scheduled_shell_argv".to_string(),
        privileged: true,
        status: "approval_required".to_string(),
        target_count: 1,
        payload_hash: payload_hash.clone(),
        created_at: unix_now().to_string(),
        completed_at: None,
    });
    memory.job_targets.write().await.push(JobTargetView {
        job_id,
        client_id: "client-a".to_string(),
        status: "approval_required".to_string(),
        exit_code: None,
        started_at: None,
        completed_at: None,
    });
    memory.scheduled_jobs.write().await.insert(
        job_id,
        ScheduledJobDispatchRecord {
            job_id,
            source_schedule_id: None,
            actor_id: Some(Uuid::nil()),
            command_type: "scheduled_shell_argv".to_string(),
            operation,
            payload_hash: payload_hash.clone(),
            targets: vec!["client-a".to_string()],
        },
    );
    let state = AppState {
        repo: repo.clone(),
        events: broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    };

    let Json(response) = cancel_job(
        State(state.clone()),
        HeaderMap::new(),
        Path(job_id),
        Json(CancelJobRequest {
            confirmed: true,
            reason: Some("operator canceled stale approval".to_string()),
        }),
    )
    .await
    .unwrap();
    let dispatch_error = dispatch_scheduled_job(
        State(state),
        HeaderMap::new(),
        Path(job_id),
        Json(DispatchScheduledJobRequest {
            confirmed: true,
            timeout_secs: None,
            force_unprivileged: false,
            envelope: None,
            envelopes: HashMap::new(),
        }),
    )
    .await
    .unwrap_err();
    let job = repo.get_job(job_id).await.unwrap().unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert!(response.canceled);
    assert_eq!(response.status, "canceled");
    assert_eq!(response.canceled_targets, 1);
    assert_eq!(job.status, "canceled");
    assert_eq!(targets[0].status, "canceled");
    assert_eq!(dispatch_error.status, axum::http::StatusCode::NOT_FOUND);
    assert!(audits.iter().any(|audit| audit.action == "job.canceled"));
}

fn proof_envelope(job_id: Uuid, client_id: &str, payload_hash_hex: &str) -> CommandEnvelope {
    let proof_key = derive_super_key("correct horse", &[1, 2, 3, 4]);
    let scope = format!("client:{client_id}");
    let proof = sign_privilege_proof(
        &proof_key,
        job_id,
        &scope,
        payload_hash_hex,
        &random_nonce(),
        unix_now() + 300,
    );
    CommandEnvelope {
        command_id: job_id,
        scope,
        payload_hash_hex: payload_hash_hex.to_string(),
        proof: Some(proof),
        server_signature: Vec::new(),
    }
}

fn scheduled_network_rollback_command() -> JobCommand {
    JobCommand::NetworkRollback {
        plan: Box::new(
            plan_tunnel(&TunnelPlanInput {
                name: "edge-a-edge-b".to_string(),
                interface_name: "tunab".to_string(),
                kind: TunnelKind::Gre,
                runtime_control: Default::default(),
                runtime_topology: Default::default(),
                left_client_id: "client-a".to_string(),
                right_client_id: "client-b".to_string(),
                left_underlay: "198.51.100.10".to_string(),
                right_underlay: "203.0.113.20".to_string(),
                address_pool_cidr: "10.255.0.0/30".to_string(),
                reserved_addresses: Vec::new(),
                bandwidth: BandwidthTier::M100,
                latency_ms: 18.0,
                packet_loss_ratio: 0.0,
                preference: 1.0,
                ospf_policy: OspfCostPolicy::default(),
            })
            .unwrap(),
        ),
        side: TunnelEndpointSide::Left,
    }
}

async fn spawn_fake_gateway_once() -> (String, tokio::task::JoinHandle<GatewayCommandDispatch>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 1024];
        let dispatch = loop {
            let read = stream.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "gateway client closed before request body");
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(dispatch) = parse_gateway_dispatch(&buffer) {
                break dispatch;
            }
        };
        let body = serde_json::to_vec(&GatewayCommandDispatchResult {
            client_id: dispatch.client_id.clone(),
            job_id: dispatch.request.job_id,
            accepted: true,
            message: "ok".to_string(),
            outputs: vec![CommandOutput {
                job_id: dispatch.request.job_id,
                stream: OutputStream::Status,
                data: Vec::new(),
                exit_code: Some(0),
                done: true,
            }],
        })
        .unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
        dispatch
    });
    (format!("http://{addr}"), task)
}

fn parse_gateway_dispatch(buffer: &[u8]) -> Option<GatewayCommandDispatch> {
    let header_end = buffer.windows(4).position(|window| window == b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&buffer[..header_end]).ok()?;
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .and_then(|value| value.trim().parse::<usize>().ok())?;
    let body_start = header_end + 4;
    if buffer.len() < body_start + content_length {
        return None;
    }
    serde_json::from_slice(&buffer[body_start..body_start + content_length]).ok()
}
