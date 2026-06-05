use std::collections::HashMap;

use crate::model_rollout_policies::CreateAgentUpdateRolloutPolicyRequest;
use crate::routes_rollout_policies::validate_create_agent_update_rollout_policy;
use crate::routes_rollouts::{
    validate_agent_update_activation_delegation_request,
    validate_agent_update_rollback_delegation_request,
    validate_agent_update_rollout_control_request,
};
use crate::*;
use ed25519_dalek::SigningKey;
use vpsman_common::AgentUpdateHeartbeat;
use vpsman_common::{
    derive_super_key, encode_json, payload_hash, random_nonce, sign_privilege_proof,
    sign_update_artifact_hash, CommandEnvelope, JobCommand,
};

fn rollback_proof_envelope(client_id: &str, command_hash: &str, ttl_secs: u64) -> CommandEnvelope {
    let command_id = Uuid::new_v4();
    let scope = format!("client:{client_id}");
    let nonce = random_nonce();
    let expires_unix = unix_now().saturating_add(ttl_secs);
    let proof = sign_privilege_proof(
        &derive_super_key("correct horse", &[1, 2, 3, 4]),
        command_id,
        &scope,
        command_hash,
        &nonce,
        expires_unix,
    );
    CommandEnvelope {
        command_id,
        scope,
        payload_hash_hex: command_hash.to_string(),
        proof: Some(proof),
        server_signature: Vec::new(),
    }
}

fn rollout_test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    }
}

fn rollout_policy_request(
    name: &str,
    scope_kind: &str,
    scope_value: Option<&str>,
    channel: Option<&str>,
    canary_count: Option<i32>,
    automation_health_gate: Option<&str>,
    priority: i32,
) -> CreateAgentUpdateRolloutPolicyRequest {
    CreateAgentUpdateRolloutPolicyRequest {
        name: name.to_string(),
        scope_kind: scope_kind.to_string(),
        scope_value: scope_value.map(str::to_string),
        channel: channel.map(str::to_string),
        canary_count,
        automation_health_gate: automation_health_gate.map(str::to_string),
        priority,
        enabled: true,
        notes: None,
        confirmed: true,
    }
}

#[test]
fn agent_update_rollout_policy_validation_rejects_ambiguous_scope_and_gate() {
    assert!(
        validate_create_agent_update_rollout_policy(&rollout_policy_request(
            "default",
            "global",
            None,
            None,
            Some(1),
            Some("heartbeat_verified"),
            0,
        ))
        .is_ok()
    );
    assert_eq!(
        validate_create_agent_update_rollout_policy(&rollout_policy_request(
            "bad",
            "global",
            Some("provider-a"),
            None,
            Some(1),
            Some("heartbeat_verified"),
            0,
        ))
        .unwrap_err()
        .code,
        "agent_update_rollout_policy_global_scope_value_forbidden"
    );
    assert_eq!(
        validate_create_agent_update_rollout_policy(&rollout_policy_request(
            "bad",
            "provider",
            None,
            None,
            Some(1),
            Some("heartbeat_verified"),
            0,
        ))
        .unwrap_err()
        .code,
        "agent_update_rollout_policy_scope_value_required"
    );
    assert_eq!(
        validate_create_agent_update_rollout_policy(&rollout_policy_request(
            "bad",
            "provider",
            Some("provider-a"),
            None,
            Some(1),
            Some("unsafe_gate"),
            0,
        ))
        .unwrap_err()
        .code,
        "agent_update_rollout_policy_health_gate_invalid"
    );
}

#[tokio::test]
async fn agent_update_rollout_policy_defaults_match_provider_channel_and_record_provenance() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = rollout_test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "edge-a".to_string(),
            display_name: "edge-a".to_string(),
            status: "connected".to_string(),
            tags: vec!["bgp".to_string(), "provider:hetzner".to_string()],
            capabilities: Default::default(),
        });
        memory.agents.write().await.push(AgentView {
            id: "edge-b".to_string(),
            display_name: "edge-b".to_string(),
            status: "connected".to_string(),
            tags: vec!["bgp".to_string(), "provider:hetzner".to_string()],
            capabilities: Default::default(),
        });
    }

    repo.upsert_agent_update_rollout_policy(
        &rollout_policy_request(
            "global-default",
            "global",
            None,
            None,
            Some(1),
            Some("manual_only"),
            0,
        ),
        &operator,
    )
    .await
    .unwrap();
    let provider_policy = repo
        .upsert_agent_update_rollout_policy(
            &rollout_policy_request(
                "hetzner-stable",
                "provider",
                Some("hetzner"),
                Some("stable"),
                Some(2),
                Some("manual_after_canary"),
                10,
            ),
            &operator,
        )
        .await
        .unwrap();

    let signing_key = SigningKey::from_bytes(&[91_u8; 32]);
    let signing_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
    let sha256_hex = "ad".repeat(32);
    let signature_hex = hex::encode(sign_update_artifact_hash(&signing_key, &sha256_hex));
    repo.record_agent_update_release(
        &CreateAgentUpdateReleaseRequest {
            name: "vpsman-agent".to_string(),
            version: "9.9.9".to_string(),
            channel: "stable".to_string(),
            artifact_sha256_hex: sha256_hex.clone(),
            artifact_signature_hex: signature_hex.clone(),
            artifact_signing_key_hex: signing_key_hex.clone(),
            artifact_url: Some("https://updates.example/vpsman-agent".to_string()),
            rollback_artifact_sha256_hex: None,
            rollback_artifact_signature_hex: None,
            rollback_artifact_signing_key_hex: None,
            rollback_artifact_url: None,
            rollback_size_bytes: None,
            size_bytes: Some(1024),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    let operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: sha256_hex.clone(),
        artifact_signature_hex: Some(signature_hex),
        artifact_signing_key_hex: Some(signing_key_hex),
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["edge-a".to_string(), "edge-b".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation.clone()),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let resolved_agents = repo
        .resolve_bulk_targets(&request.target_selection())
        .await
        .unwrap()
        .targets;
    let rollout_policy = repo
        .resolve_agent_update_rollout_policy(&request, &operation, &resolved_agents)
        .await
        .unwrap();

    assert_eq!(rollout_policy.policy_id, Some(provider_policy.id));
    assert_eq!(
        rollout_policy.policy_name.as_deref(),
        Some("hetzner-stable")
    );
    assert_eq!(rollout_policy.canary_count, Some(2));
    assert_eq!(
        rollout_policy.automation_health_gate.as_deref(),
        Some("manual_after_canary")
    );

    let targets = resolved_agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect::<Vec<_>>();
    repo.record_dispatching_job_with_rollout_policy(
        &request,
        &command_hash,
        &operator,
        &targets,
        &rollout_policy,
    )
    .await
    .unwrap();
    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();

    assert_eq!(rollouts[0].canary_count, 2);
    assert_eq!(rollouts[0].rollout_policy_id, Some(provider_policy.id));
    assert_eq!(
        rollouts[0].rollout_policy_name.as_deref(),
        Some("hetzner-stable")
    );
    assert_eq!(rollouts[0].automation_health_gate, "manual_after_canary");
}

#[tokio::test]
async fn agent_update_dispatch_records_rollout_without_sensitive_artifact_url() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let artifact_url = "https://updates.example/private/vpsman-agent?token=secret";
    let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
    let signing_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
    let sha256_hex = "ab".repeat(32);
    let operation = JobCommand::UpdateAgent {
        artifact_url: artifact_url.to_string(),
        sha256_hex: sha256_hex.clone(),
        artifact_signature_hex: Some(hex::encode(sign_update_artifact_hash(
            &signing_key,
            &sha256_hex,
        ))),
        artifact_signing_key_hex: Some(signing_key_hex.clone()),
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(30),
        canary_count: Some(1),
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let targets = vec!["client-a".to_string()];

    let job_id = repo
        .record_dispatching_job(&request, &command_hash, &operator, &targets)
        .await
        .unwrap();
    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();

    assert_eq!(rollouts.len(), 1);
    assert_eq!(rollouts[0].job_id, job_id);
    assert_eq!(rollouts[0].status, "staging_requested");
    assert_eq!(rollouts[0].target_count, 1);
    assert_eq!(rollouts[0].canary_count, 1);
    assert_eq!(rollouts[0].pending_count, 1);
    assert_eq!(rollouts[0].artifact_sha256_hex, "ab".repeat(32));
    assert!(rollouts[0].artifact_signature_provided);
    assert_ne!(
        rollouts[0].artifact_signing_key_sha256_hex.as_deref(),
        Some(signing_key_hex.as_str())
    );
    let rollout_json = serde_json::to_string(&rollouts).unwrap();
    assert!(!rollout_json.contains(artifact_url));
    assert!(!rollout_json.contains("secret"));

    repo.update_job_target_result(
        job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "staged".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    repo.finish_job(job_id, "completed").await.unwrap();

    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();
    assert_eq!(rollouts[0].status, "staged");
    assert_eq!(rollouts[0].completed_count, 1);
    assert_eq!(rollouts[0].failed_count, 0);
    assert_eq!(rollouts[0].pending_count, 0);
    assert_eq!(rollouts[0].targets[0].status, "completed");
    assert_eq!(rollouts[0].targets[0].exit_code, Some(0));
}

#[tokio::test]
async fn agent_update_rollout_control_updates_pause_gate_and_audits() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "aa".repeat(32),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(30),
        canary_count: Some(1),
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    repo.record_dispatching_job(
        &request,
        &command_hash,
        &operator,
        &["client-a".to_string()],
    )
    .await
    .unwrap();
    let rollout_id = repo.list_agent_update_rollouts(10).await.unwrap()[0].id;

    let paused = repo
        .update_agent_update_rollout_control(
            rollout_id,
            &AgentUpdateRolloutControlRequest {
                confirmed: true,
                paused: Some(true),
                pause_reason: Some("maintenance window".to_string()),
                automation_health_gate: Some("manual_after_canary".to_string()),
            },
            &operator,
        )
        .await
        .unwrap();

    assert!(paused.automation_paused);
    assert_eq!(
        paused.automation_pause_reason.as_deref(),
        Some("maintenance window")
    );
    assert_eq!(paused.automation_health_gate, "manual_after_canary");
    assert_eq!(paused.automation_status, "paused");
    assert_eq!(paused.automation_next_action, None);
    assert!(paused.automation_targets.is_empty());

    let resumed = repo
        .update_agent_update_rollout_control(
            rollout_id,
            &AgentUpdateRolloutControlRequest {
                confirmed: true,
                paused: Some(false),
                pause_reason: None,
                automation_health_gate: Some("heartbeat_verified".to_string()),
            },
            &operator,
        )
        .await
        .unwrap();

    assert!(!resumed.automation_paused);
    assert_eq!(resumed.automation_pause_reason, None);
    assert_eq!(resumed.automation_health_gate, "heartbeat_verified");
    assert_eq!(resumed.automation_status, "unreconciled");
    assert!(
        repo.list_audit_logs(10)
            .await
            .unwrap()
            .iter()
            .filter(|audit| audit.action == "agent_update.rollout_control_updated")
            .count()
            >= 2
    );
}

#[test]
fn agent_update_rollout_control_validation_requires_confirmed_safe_gate() {
    assert!(
        validate_agent_update_rollout_control_request(&AgentUpdateRolloutControlRequest {
            confirmed: false,
            paused: Some(true),
            pause_reason: None,
            automation_health_gate: None,
        })
        .is_err()
    );
    assert!(
        validate_agent_update_rollout_control_request(&AgentUpdateRolloutControlRequest {
            confirmed: true,
            paused: None,
            pause_reason: None,
            automation_health_gate: Some("dispatch_without_proof".to_string()),
        })
        .is_err()
    );
    assert!(
        validate_agent_update_rollout_control_request(&AgentUpdateRolloutControlRequest {
            confirmed: true,
            paused: Some(true),
            pause_reason: Some("reason".to_string()),
            automation_health_gate: Some("manual_only".to_string()),
        })
        .is_ok()
    );
}

#[tokio::test]
async fn agent_update_rollback_delegation_records_and_claims_timeout_targets_only() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let update_operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ef".repeat(32),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let update_hash = payload_hash(&encode_json(&update_operation).unwrap());
    let update_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(update_operation),
        timeout_secs: Some(30),
        canary_count: Some(1),
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    repo.record_dispatching_job(
        &update_request,
        &update_hash,
        &operator,
        &["client-a".to_string()],
    )
    .await
    .unwrap();
    let rollout_id = repo.list_agent_update_rollouts(10).await.unwrap()[0].id;

    let rollback_operation = JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: Some("fe".repeat(32)),
    };
    let command_hash = payload_hash(&encode_json(&rollback_operation).unwrap());
    let envelope = rollback_proof_envelope("client-a", &command_hash, 600);
    let proof_hex = envelope.proof.as_ref().unwrap().proof_hex.clone();
    let mut envelopes = HashMap::new();
    envelopes.insert("client-a".to_string(), envelope);
    let request = AgentUpdateRollbackDelegationRequest {
        confirmed: true,
        rollback_sha256_hex: Some("fe".repeat(32)),
        force_unprivileged: false,
        envelopes,
    };

    let summary = repo
        .record_agent_update_rollback_delegation(rollout_id, &request, &operator)
        .await
        .unwrap();

    assert_eq!(summary.rollout_id, rollout_id);
    assert_eq!(summary.action, "agent_update_rollback");
    assert_eq!(summary.target_count, 1);
    assert_eq!(summary.ready_count, 1);
    assert_eq!(summary.payload_hash, command_hash);
    assert!(summary.proof_expires_unix_min.is_some());
    let listed = repo.list_agent_update_rollouts(10).await.unwrap();
    assert_eq!(listed[0].rollback_delegations.len(), 1);
    assert_eq!(listed[0].rollback_delegations[0].payload_hash, command_hash);
    assert_eq!(listed[0].rollback_delegations[0].ready_count, 1);
    assert!(repo
        .claim_ready_agent_update_rollback_delegations(10)
        .await
        .unwrap()
        .is_empty());

    {
        let mut rollouts = memory.agent_update_rollouts.write().await;
        let rollout = rollouts
            .iter_mut()
            .find(|rollout| rollout.id == rollout_id)
            .unwrap();
        rollout.status = "heartbeat_timeout".to_string();
        rollout.failed_count = 1;
        rollout.pending_count = 0;
        rollout.updated_at = unix_now().to_string();
        let target = rollout
            .targets
            .iter_mut()
            .find(|target| target.client_id == "client-a")
            .unwrap();
        target.status = "heartbeat_timeout".to_string();
        target.updated_at = unix_now().to_string();
    }

    let claims = repo
        .claim_ready_agent_update_rollback_delegations(10)
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].rollout_id, rollout_id);
    assert_eq!(claims[0].payload_hash, command_hash);
    assert_eq!(claims[0].clients, vec!["client-a"]);
    assert_eq!(claims[0].envelopes.len(), 1);

    let summary = repo
        .agent_update_rollback_delegation_summary(rollout_id, &command_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(summary.ready_count, 0);
    assert_eq!(summary.dispatching_count, 1);

    let audit_json = serde_json::to_string(&repo.list_audit_logs(20).await.unwrap()).unwrap();
    assert!(audit_json.contains("scoped_exact_rollback_proof_escrow"));
    assert!(!audit_json.contains(&proof_hex));
}

#[tokio::test]
async fn agent_update_activation_delegation_claims_recommended_completed_targets_only() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let staged_sha256_hex = "ef".repeat(32);
    let update_operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: staged_sha256_hex.clone(),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let update_hash = payload_hash(&encode_json(&update_operation).unwrap());
    let update_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string(), "client-b".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(update_operation),
        timeout_secs: Some(30),
        canary_count: Some(1),
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    repo.record_dispatching_job(
        &update_request,
        &update_hash,
        &operator,
        &["client-a".to_string(), "client-b".to_string()],
    )
    .await
    .unwrap();
    let rollout_id = repo.list_agent_update_rollouts(10).await.unwrap()[0].id;

    let activation_operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: staged_sha256_hex.clone(),
        restart_agent: true,
    };
    let command_hash = payload_hash(&encode_json(&activation_operation).unwrap());
    let envelope_a = rollback_proof_envelope("client-a", &command_hash, 600);
    let proof_hex = envelope_a.proof.as_ref().unwrap().proof_hex.clone();
    let mut envelopes = HashMap::new();
    envelopes.insert("client-a".to_string(), envelope_a);
    envelopes.insert(
        "client-b".to_string(),
        rollback_proof_envelope("client-b", &command_hash, 600),
    );
    let request = AgentUpdateActivationDelegationRequest {
        confirmed: true,
        restart_agent: true,
        force_unprivileged: false,
        envelopes,
    };

    let summary = repo
        .record_agent_update_activation_delegation(rollout_id, &request, &operator)
        .await
        .unwrap();

    assert_eq!(summary.rollout_id, rollout_id);
    assert_eq!(summary.action, "agent_update_activate");
    assert_eq!(summary.target_count, 2);
    assert_eq!(summary.ready_count, 2);
    assert_eq!(summary.payload_hash, command_hash);
    assert_eq!(summary.staged_sha256_hex, staged_sha256_hex);
    assert!(summary.restart_agent);
    let listed = repo.list_agent_update_rollouts(10).await.unwrap();
    assert_eq!(listed[0].activation_delegations.len(), 1);
    assert_eq!(
        listed[0].activation_delegations[0].payload_hash,
        command_hash
    );
    assert_eq!(listed[0].activation_delegations[0].ready_count, 2);
    assert!(repo
        .claim_ready_agent_update_activation_delegations(10)
        .await
        .unwrap()
        .is_empty());

    {
        let mut rollouts = memory.agent_update_rollouts.write().await;
        let rollout = rollouts
            .iter_mut()
            .find(|rollout| rollout.id == rollout_id)
            .unwrap();
        rollout.status = "staged".to_string();
        rollout.completed_count = 2;
        rollout.pending_count = 0;
        rollout.automation_status = "ready_activate_canary".to_string();
        rollout.automation_next_action = Some("operator_activate_batch".to_string());
        rollout.automation_targets = vec!["client-a".to_string()];
        rollout.updated_at = unix_now().to_string();
        for target in &mut rollout.targets {
            target.status = "completed".to_string();
            target.updated_at = unix_now().to_string();
        }
    }

    let claims = repo
        .claim_ready_agent_update_activation_delegations(10)
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].rollout_id, rollout_id);
    assert_eq!(claims[0].payload_hash, command_hash);
    assert_eq!(claims[0].clients, vec!["client-a"]);
    assert_eq!(claims[0].staged_sha256_hex, "ef".repeat(32));
    assert!(claims[0].restart_agent);
    assert_eq!(claims[0].envelopes.len(), 1);

    let summary = repo
        .agent_update_activation_delegation_summary(rollout_id, &command_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(summary.ready_count, 1);
    assert_eq!(summary.dispatching_count, 1);

    let audit_json = serde_json::to_string(&repo.list_audit_logs(20).await.unwrap()).unwrap();
    assert!(audit_json.contains("scoped_exact_activation_proof_escrow"));
    assert!(!audit_json.contains(&proof_hex));
}

#[tokio::test]
async fn agent_update_delegated_proof_expiry_updates_rollout_read_model() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let staged_sha256_hex = "ef".repeat(32);
    let update_operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: staged_sha256_hex.clone(),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let update_hash = payload_hash(&encode_json(&update_operation).unwrap());
    let update_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(update_operation),
        timeout_secs: Some(30),
        canary_count: Some(1),
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    repo.record_dispatching_job(
        &update_request,
        &update_hash,
        &operator,
        &["client-a".to_string()],
    )
    .await
    .unwrap();
    let rollout_id = repo.list_agent_update_rollouts(10).await.unwrap()[0].id;

    let activation_operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex,
        restart_agent: false,
    };
    let command_hash = payload_hash(&encode_json(&activation_operation).unwrap());
    let envelope = rollback_proof_envelope("client-a", &command_hash, 600);
    let proof_hex = envelope.proof.as_ref().unwrap().proof_hex.clone();
    let mut envelopes = HashMap::new();
    envelopes.insert("client-a".to_string(), envelope);
    repo.record_agent_update_activation_delegation(
        rollout_id,
        &AgentUpdateActivationDelegationRequest {
            confirmed: true,
            restart_agent: false,
            force_unprivileged: false,
            envelopes,
        },
        &operator,
    )
    .await
    .unwrap();

    {
        let mut delegations = memory.agent_update_rollback_delegations.write().await;
        let record = delegations
            .iter_mut()
            .find(|record| record.rollout_id == rollout_id)
            .unwrap();
        record.proof_expires_unix = unix_now() as i64 - 1;
    }

    let expired = repo
        .expire_agent_update_delegated_proofs(100)
        .await
        .unwrap();
    assert_eq!(expired, 1);

    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();
    let delegation = &rollouts[0].activation_delegations[0];
    assert_eq!(delegation.ready_count, 0);
    assert_eq!(delegation.expired_count, 1);

    let claims = repo
        .claim_ready_agent_update_activation_delegations(10)
        .await
        .unwrap();
    assert!(claims.is_empty());

    let audit_json = serde_json::to_string(&repo.list_audit_logs(20).await.unwrap()).unwrap();
    assert!(audit_json.contains("agent_update.delegated_proof_expired"));
    assert!(audit_json.contains("proof_expires_unix elapsed before dispatch claim"));
    assert!(!audit_json.contains(&proof_hex));
}

#[test]
fn agent_update_rollback_delegation_validation_requires_confirmed_envelopes_and_hash_shape() {
    assert!(validate_agent_update_rollback_delegation_request(
        &AgentUpdateRollbackDelegationRequest {
            confirmed: false,
            rollback_sha256_hex: None,
            force_unprivileged: false,
            envelopes: HashMap::new(),
        }
    )
    .is_err());
    assert!(validate_agent_update_rollback_delegation_request(
        &AgentUpdateRollbackDelegationRequest {
            confirmed: true,
            rollback_sha256_hex: None,
            force_unprivileged: false,
            envelopes: HashMap::new(),
        }
    )
    .is_err());
    let mut envelopes = HashMap::new();
    envelopes.insert(
        "client-a".to_string(),
        rollback_proof_envelope("client-a", &"aa".repeat(32), 600),
    );
    assert!(validate_agent_update_rollback_delegation_request(
        &AgentUpdateRollbackDelegationRequest {
            confirmed: true,
            rollback_sha256_hex: Some("bad".to_string()),
            force_unprivileged: false,
            envelopes,
        }
    )
    .is_err());
}

#[test]
fn agent_update_activation_delegation_validation_requires_confirmed_envelopes() {
    assert!(validate_agent_update_activation_delegation_request(
        &AgentUpdateActivationDelegationRequest {
            confirmed: false,
            restart_agent: false,
            force_unprivileged: false,
            envelopes: HashMap::new(),
        }
    )
    .is_err());
    assert!(validate_agent_update_activation_delegation_request(
        &AgentUpdateActivationDelegationRequest {
            confirmed: true,
            restart_agent: false,
            force_unprivileged: false,
            envelopes: HashMap::new(),
        }
    )
    .is_err());
    let mut envelopes = HashMap::new();
    envelopes.insert(
        "client-a".to_string(),
        rollback_proof_envelope("client-a", &"aa".repeat(32), 600),
    );
    assert!(validate_agent_update_activation_delegation_request(
        &AgentUpdateActivationDelegationRequest {
            confirmed: true,
            restart_agent: true,
            force_unprivileged: false,
            envelopes,
        }
    )
    .is_ok());
}

#[tokio::test]
async fn agent_update_heartbeat_marks_rollout_after_restart() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "cd".repeat(32),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let job_id = repo
        .record_dispatching_job(
            &request,
            &command_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "staged".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    repo.finish_job(job_id, "completed").await.unwrap();

    repo.record_agent_update_heartbeat(
        "client-a",
        &AgentUpdateHeartbeat {
            activation_job_id: Uuid::new_v4(),
            sha256_hex: "cd".repeat(32),
            marker_unix: 1_780_000_000,
            observed_unix: 1_780_000_030,
        },
    )
    .await
    .unwrap();

    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();
    assert_eq!(rollouts[0].status, "heartbeat_verified");
    assert_eq!(rollouts[0].completed_count, 1);
    assert_eq!(rollouts[0].targets[0].status, "heartbeat_verified");
    assert_eq!(rollouts[0].targets[0].exit_code, Some(0));
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "agent_update.heartbeat_verified"));
}

#[tokio::test]
async fn agent_update_activation_completed_marks_rollout_pending_restart() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let staged_sha256_hex = "de".repeat(32);
    let update_operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: staged_sha256_hex.clone(),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let update_hash = payload_hash(&encode_json(&update_operation).unwrap());
    let update_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(update_operation),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let update_job_id = repo
        .record_dispatching_job(
            &update_request,
            &update_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        update_job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "staged".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    repo.finish_job(update_job_id, "completed").await.unwrap();

    let activation_operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: staged_sha256_hex.clone(),
        restart_agent: false,
    };
    let activation_hash = payload_hash(&encode_json(&activation_operation).unwrap());
    let activation_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update_activate".to_string(),
        argv: Vec::new(),
        operation: Some(activation_operation),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let activation_job_id = repo
        .record_dispatching_job(
            &activation_request,
            &activation_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        activation_job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "activated".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    repo.finish_job(activation_job_id, "completed")
        .await
        .unwrap();

    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();
    assert_eq!(rollouts[0].status, "activation_pending_restart");
    assert_eq!(rollouts[0].completed_count, 0);
    assert_eq!(rollouts[0].pending_count, 1);
    assert_eq!(rollouts[0].targets[0].status, "activation_pending_restart");
    assert_eq!(rollouts[0].targets[0].exit_code, Some(0));
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "agent_update.activation_pending_restart"));
}

#[tokio::test]
async fn agent_update_rollback_completed_marks_rollout_rolled_back() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let staged_sha256_hex = "da".repeat(32);
    let rollback_sha256_hex = "ad".repeat(32);
    let update_operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: staged_sha256_hex.clone(),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let update_hash = payload_hash(&encode_json(&update_operation).unwrap());
    let update_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(update_operation),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let update_job_id = repo
        .record_dispatching_job(
            &update_request,
            &update_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        update_job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "staged".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    repo.finish_job(update_job_id, "completed").await.unwrap();

    let activation_operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: staged_sha256_hex.clone(),
        restart_agent: false,
    };
    let activation_hash = payload_hash(&encode_json(&activation_operation).unwrap());
    let activation_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update_activate".to_string(),
        argv: Vec::new(),
        operation: Some(activation_operation),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let activation_job_id = repo
        .record_dispatching_job(
            &activation_request,
            &activation_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        activation_job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "activated".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    repo.finish_job(activation_job_id, "completed")
        .await
        .unwrap();

    let rollback_operation = JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: Some(rollback_sha256_hex.clone()),
    };
    let rollback_hash = payload_hash(&encode_json(&rollback_operation).unwrap());
    let rollback_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update_rollback".to_string(),
        argv: Vec::new(),
        operation: Some(rollback_operation),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let rollback_job_id = repo
        .record_dispatching_job(
            &rollback_request,
            &rollback_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        rollback_job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "rolled back".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    repo.finish_job(rollback_job_id, "completed").await.unwrap();

    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();
    assert_eq!(rollouts[0].status, "rolled_back");
    assert_eq!(rollouts[0].completed_count, 1);
    assert_eq!(rollouts[0].failed_count, 0);
    assert_eq!(rollouts[0].pending_count, 0);
    assert_eq!(rollouts[0].targets[0].status, "rolled_back");
    assert_eq!(rollouts[0].targets[0].exit_code, Some(0));
    let audits = repo.list_audit_logs(20).await.unwrap();
    let rollback_audit = audits
        .iter()
        .find(|audit| audit.action == "agent_update.rollback_completed")
        .expect("rollback completion audit");
    assert_eq!(
        rollback_audit
            .metadata
            .get("rollback_sha256_hex")
            .and_then(|value| value.as_str()),
        Some(rollback_sha256_hex.as_str())
    );
    assert_eq!(
        rollback_audit
            .metadata
            .get("previous_status")
            .and_then(|value| value.as_str()),
        Some("activation_pending_restart")
    );
}

#[tokio::test]
async fn stale_activation_pending_rollout_expires_with_audit() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let staged_sha256_hex = "fa".repeat(32);
    let update_operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: staged_sha256_hex.clone(),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let update_hash = payload_hash(&encode_json(&update_operation).unwrap());
    let update_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(update_operation),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let update_job_id = repo
        .record_dispatching_job(
            &update_request,
            &update_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        update_job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "staged".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    repo.finish_job(update_job_id, "completed").await.unwrap();

    let activation_operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: staged_sha256_hex.clone(),
        restart_agent: false,
    };
    let activation_hash = payload_hash(&encode_json(&activation_operation).unwrap());
    let activation_request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update_activate".to_string(),
        argv: Vec::new(),
        operation: Some(activation_operation),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let activation_job_id = repo
        .record_dispatching_job(
            &activation_request,
            &activation_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        activation_job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "activated".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();

    {
        let mut rollouts = memory.agent_update_rollouts.write().await;
        rollouts[0].heartbeat_timeout_secs = Some(10);
        rollouts[0].targets[0].updated_at = crate::unix_now().saturating_sub(60).to_string();
    }

    let expired = repo
        .expire_agent_update_heartbeat_timeouts(900)
        .await
        .unwrap();

    assert_eq!(expired, 1);
    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();
    assert_eq!(rollouts[0].status, "heartbeat_timeout");
    assert_eq!(rollouts[0].completed_count, 0);
    assert_eq!(rollouts[0].failed_count, 1);
    assert_eq!(rollouts[0].pending_count, 0);
    assert_eq!(rollouts[0].targets[0].status, "heartbeat_timeout");
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "agent_update.heartbeat_timeout"));
}

#[tokio::test]
async fn agent_update_heartbeat_does_not_upgrade_failed_rollout_target() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ef".repeat(32),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(30),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let job_id = repo
        .record_dispatching_job(
            &request,
            &command_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "failed".to_string(),
            exit_code: Some(1),
            accepted: true,
            message: "staging failed".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    repo.finish_job(job_id, "dispatch_failed").await.unwrap();

    repo.record_agent_update_heartbeat(
        "client-a",
        &AgentUpdateHeartbeat {
            activation_job_id: Uuid::new_v4(),
            sha256_hex: "ef".repeat(32),
            marker_unix: 1_780_000_000,
            observed_unix: 1_780_000_030,
        },
    )
    .await
    .unwrap();

    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();
    assert_eq!(rollouts[0].status, "dispatch_failed");
    assert_eq!(rollouts[0].failed_count, 1);
    assert_eq!(rollouts[0].targets[0].status, "failed");
    assert_eq!(rollouts[0].targets[0].exit_code, Some(1));
    assert!(!repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "agent_update.heartbeat_verified"));
}
