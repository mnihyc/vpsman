use std::collections::HashMap;

use crate::*;
use vpsman_common::{
    derive_super_key, encode_json, payload_hash, random_nonce, sign_privilege_proof,
    CommandEnvelope, JobCommand,
};

fn operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    }
}

fn proof_envelope(client_id: &str, command_hash: &str, ttl_secs: u64) -> CommandEnvelope {
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

fn agent_update_request(operation: JobCommand, clients: Vec<String>) -> CreateJobRequest {
    CreateJobRequest {
        targets: Vec::new(),
        clients,
        pools: Vec::new(),
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
    }
}

#[tokio::test]
async fn failed_activation_marks_rollout_and_allows_delegated_rollback_claim() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = operator();
    let staged_sha256_hex = "ce".repeat(32);
    let update_operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: staged_sha256_hex.clone(),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let update_hash = payload_hash(&encode_json(&update_operation).unwrap());
    let update_request = agent_update_request(update_operation, vec!["client-a".to_string()]);
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
    let rollout_id = repo.list_agent_update_rollouts(10).await.unwrap()[0].id;

    let rollback_operation = JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: None,
    };
    let rollback_hash = payload_hash(&encode_json(&rollback_operation).unwrap());
    let mut rollback_envelopes = HashMap::new();
    rollback_envelopes.insert(
        "client-a".to_string(),
        proof_envelope("client-a", &rollback_hash, 600),
    );
    repo.record_agent_update_rollback_delegation(
        rollout_id,
        &AgentUpdateRollbackDelegationRequest {
            confirmed: true,
            rollback_sha256_hex: None,
            force_unprivileged: true,
            envelopes: rollback_envelopes,
        },
        &operator,
    )
    .await
    .unwrap();

    let activation_operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex,
        restart_agent: true,
    };
    let activation_hash = payload_hash(&encode_json(&activation_operation).unwrap());
    let activation_request =
        agent_update_request(activation_operation, vec!["client-a".to_string()]);
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
            status: "failed".to_string(),
            exit_code: Some(70),
            accepted: true,
            message: "activation command failed after staging".to_string(),
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();

    let rollouts = repo.list_agent_update_rollouts(10).await.unwrap();
    assert_eq!(rollouts[0].status, "activation_failed");
    assert_eq!(rollouts[0].failed_count, 1);
    assert_eq!(rollouts[0].pending_count, 0);
    assert_eq!(rollouts[0].targets[0].status, "activation_failed");
    assert_eq!(rollouts[0].targets[0].exit_code, Some(70));

    let claims = repo
        .claim_ready_agent_update_rollback_delegations(10)
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].rollout_id, rollout_id);
    assert_eq!(claims[0].clients, vec!["client-a"]);
    assert!(claims[0].force_unprivileged);

    let audits = repo.list_audit_logs(20).await.unwrap();
    let activation_audit = audits
        .iter()
        .find(|audit| audit.action == "agent_update.activation_failed")
        .expect("activation failure audit");
    assert_eq!(
        activation_audit
            .metadata
            .get("rollback_recommended")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        activation_audit
            .metadata
            .get("activation_outcome_status")
            .and_then(|value| value.as_str()),
        Some("failed")
    );
}
