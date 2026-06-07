use std::{collections::HashMap, path::PathBuf};

use crate::state::UpdateReleasePolicy;
use crate::*;
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use ed25519_dalek::SigningKey;
use vpsman_common::{encode_json, payload_hash, sign_update_artifact_hash, AgentHello, JobCommand};

#[tokio::test]
async fn agent_update_release_registry_records_sanitized_signed_metadata() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let request = signed_release_request("vpsman-agent", "1.2.3", "stable");

    routes_update_releases::validate_agent_update_release_request(&request).unwrap();
    let release = repo
        .record_agent_update_release(&request, &operator)
        .await
        .unwrap();

    assert_eq!(release.name, "vpsman-agent");
    assert_eq!(release.version, "1.2.3");
    assert_eq!(release.channel, "stable");
    assert_eq!(release.status, "published_metadata_only");
    assert_eq!(release.artifact_sha256_hex, request.artifact_sha256_hex);
    assert!(release.artifact_signature_provided);
    assert!(release.artifact_signature_sha256_hex.is_some());
    assert!(release.artifact_url_sha256_hex.is_some());
    assert_ne!(
        release.artifact_signing_key_sha256_hex,
        request.artifact_signing_key_hex
    );
    assert!(repo
        .agent_update_release_exists_for_artifact(
            &request.artifact_sha256_hex,
            Some(&request.artifact_signing_key_hex),
        )
        .await
        .unwrap());
    let serialized =
        serde_json::to_string(&repo.list_agent_update_releases(10).await.unwrap()).unwrap();
    assert!(!serialized.contains("https://updates.example"));
    assert!(!serialized.contains(&request.artifact_signature_hex));
    assert!(!serialized.contains(&request.artifact_signing_key_hex));
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "agent_update.release_recorded"));
}

#[test]
fn agent_update_release_registry_rejects_bad_or_unconfirmed_metadata() {
    let mut request = signed_release_request("vpsman-agent", "1.2.3", "stable");
    request.confirmed = false;
    assert_eq!(
        routes_update_releases::validate_agent_update_release_request(&request)
            .unwrap_err()
            .code,
        "agent_update_release_confirmation_required"
    );

    let mut request = signed_release_request("vpsman-agent", "1.2.3", "stable");
    request.artifact_url = Some("http://updates.example/vpsman-agent".to_string());
    assert_eq!(
        routes_update_releases::validate_agent_update_release_request(&request)
            .unwrap_err()
            .code,
        "agent_update_release_artifact_url_invalid"
    );

    let mut request = signed_release_request("vpsman-agent", "1.2.3", "stable");
    request.artifact_signature_hex = "00".repeat(64);
    assert_eq!(
        routes_update_releases::validate_agent_update_release_request(&request)
            .unwrap_err()
            .code,
        "agent_update_release_signature_mismatch"
    );
}

#[tokio::test]
async fn strict_agent_update_release_policy_rejects_unregistered_update_before_gateway() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        repository_ingest::upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-a".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                capabilities: Default::default(),
            },
        )
        .await;
    }
    let release_request = signed_release_request("vpsman-agent", "1.2.3", "stable");
    let operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: release_request.artifact_sha256_hex,
        artifact_signature_hex: Some(release_request.artifact_signature_hex),
        artifact_signing_key_hex: Some(release_request.artifact_signing_key_hex),
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        selector_expression: "id:client-a".to_string(),
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
    let state = AppState {
        repo: repo.clone(),
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: Some(std::sync::Arc::new(SigningKey::from_bytes(&[7_u8; 32]))),
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: true,
    };

    let (status, Json(response)) =
        routes_jobs::create_job(State(state), HeaderMap::new(), Json(request))
            .await
            .unwrap();

    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(response.accepted_targets, 0);
    assert_eq!(response.status, "rejected_authorization_required");
    let jobs = repo.list_jobs(10).await.unwrap();
    assert_eq!(jobs[0].payload_hash, command_hash);
    let audits = repo.list_audit_logs(10).await.unwrap();
    assert!(audits
        .iter()
        .any(|audit| audit.action == "job.rejected_authorization_required"));
}

#[tokio::test]
async fn uploaded_agent_update_artifact_is_hosted_and_sanitized() {
    let repo = Repository::Memory(MemoryState::default());
    let signing_key = SigningKey::from_bytes(&[31_u8; 32]);
    let artifact = b"hosted-agent-update-artifact".to_vec();
    let sha256_hex = payload_hash(&artifact);
    let request = UploadAgentUpdateArtifactRequest {
        name: "vpsman-agent".to_string(),
        version: "2.0.0".to_string(),
        channel: "stable".to_string(),
        artifact_base64: BASE64_STANDARD.encode(&artifact),
        artifact_signature_hex: hex::encode(sign_update_artifact_hash(&signing_key, &sha256_hex)),
        artifact_signing_key_hex: hex::encode(signing_key.verifying_key().to_bytes()),
        rollback_artifact_base64: None,
        rollback_artifact_signature_hex: None,
        rollback_artifact_signing_key_hex: None,
        notes: Some("hosted update".to_string()),
        confirmed: true,
    };
    let store_root = std::env::temp_dir().join(format!("vpsman-update-store-{}", Uuid::new_v4()));
    let state = AppState {
        repo: repo.clone(),
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: Some(BackupObjectStore::filesystem(store_root).unwrap()),
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    };

    let (status, Json(release)) = routes_update_releases::upload_agent_update_artifact(
        State(state.clone()),
        HeaderMap::new(),
        Json(request),
    )
    .await
    .unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(release.status, "artifact_hosted");
    assert_eq!(release.artifact_sha256_hex, sha256_hex);
    assert_eq!(
        release.artifact_download_path.as_deref(),
        Some(format!("/api/v1/agent-update-artifacts/{sha256_hex}").as_str())
    );
    let serialized = serde_json::to_string(&release).unwrap();
    assert!(!serialized.contains("hosted-agent-update-artifact"));
    assert!(!serialized.contains(&hex::encode(signing_key.to_bytes())));

    let response = routes_update_releases::download_agent_update_artifact(
        State(state),
        Path(sha256_hex.clone()),
    )
    .await
    .unwrap();
    let downloaded = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(downloaded.as_ref(), artifact.as_slice());
    assert!(repo
        .agent_update_release_exists_for_artifact(
            &sha256_hex,
            Some(&hex::encode(signing_key.verifying_key().to_bytes()))
        )
        .await
        .unwrap());
    let audits = repo.list_audit_logs(10).await.unwrap();
    assert!(audits
        .iter()
        .any(|audit| audit.action == "agent_update.artifact_uploaded"));
}

#[tokio::test]
async fn release_registry_records_sanitized_rollback_bundle_metadata() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let rollback_key = SigningKey::from_bytes(&[43_u8; 32]);
    let rollback_sha256_hex = "34".repeat(32);
    let mut request = signed_release_request("vpsman-agent", "2.1.0", "stable");
    request.rollback_artifact_sha256_hex = Some(rollback_sha256_hex.clone());
    request.rollback_artifact_signature_hex = Some(hex::encode(sign_update_artifact_hash(
        &rollback_key,
        &rollback_sha256_hex,
    )));
    request.rollback_artifact_signing_key_hex =
        Some(hex::encode(rollback_key.verifying_key().to_bytes()));
    request.rollback_artifact_url = Some("https://updates.example/vpsman-agent-rollback".into());
    request.rollback_size_bytes = Some(2048);

    routes_update_releases::validate_agent_update_release_request(&request).unwrap();
    let release = repo
        .record_agent_update_release(&request, &operator)
        .await
        .unwrap();

    assert_eq!(
        release.rollback_artifact_sha256_hex.as_deref(),
        Some(rollback_sha256_hex.as_str())
    );
    assert!(release.rollback_artifact_signature_provided);
    assert!(release.rollback_artifact_signature_sha256_hex.is_some());
    assert!(release.rollback_artifact_url_sha256_hex.is_some());
    assert_ne!(
        release.rollback_artifact_signing_key_sha256_hex.as_deref(),
        request.rollback_artifact_signing_key_hex.as_deref()
    );
    let serialized = serde_json::to_string(&release).unwrap();
    assert!(!serialized.contains("vpsman-agent-rollback"));
    assert!(!serialized.contains(request.rollback_artifact_signature_hex.as_deref().unwrap()));
    assert!(!serialized.contains(
        request
            .rollback_artifact_signing_key_hex
            .as_deref()
            .unwrap()
    ));
    let audit = repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .into_iter()
        .find(|audit| audit.action == "agent_update.release_recorded")
        .unwrap();
    let audit_json = serde_json::to_string(&audit.metadata).unwrap();
    assert!(audit_json.contains("rollback_artifact_sha256_hex"));
    assert!(!audit_json.contains("vpsman-agent-rollback"));
}

#[tokio::test]
async fn uploaded_release_can_host_rollback_bundle_and_public_urls() {
    let repo = Repository::Memory(MemoryState::default());
    let signing_key = SigningKey::from_bytes(&[31_u8; 32]);
    let rollback_key = SigningKey::from_bytes(&[32_u8; 32]);
    let artifact = b"hosted-agent-update-artifact".to_vec();
    let rollback_artifact = b"hosted-agent-update-rollback-artifact".to_vec();
    let sha256_hex = payload_hash(&artifact);
    let rollback_sha256_hex = payload_hash(&rollback_artifact);
    let request = UploadAgentUpdateArtifactRequest {
        name: "vpsman-agent".to_string(),
        version: "2.2.0".to_string(),
        channel: "stable".to_string(),
        artifact_base64: BASE64_STANDARD.encode(&artifact),
        artifact_signature_hex: hex::encode(sign_update_artifact_hash(&signing_key, &sha256_hex)),
        artifact_signing_key_hex: hex::encode(signing_key.verifying_key().to_bytes()),
        rollback_artifact_base64: Some(BASE64_STANDARD.encode(&rollback_artifact)),
        rollback_artifact_signature_hex: Some(hex::encode(sign_update_artifact_hash(
            &rollback_key,
            &rollback_sha256_hex,
        ))),
        rollback_artifact_signing_key_hex: Some(hex::encode(
            rollback_key.verifying_key().to_bytes(),
        )),
        notes: Some("hosted update with rollback".to_string()),
        confirmed: true,
    };
    let store_root = std::env::temp_dir().join(format!("vpsman-update-store-{}", Uuid::new_v4()));
    let state = AppState {
        repo: repo.clone(),
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: Some(BackupObjectStore::filesystem(store_root).unwrap()),
        update_artifact_public_base_url: Some("https://updates.example".to_string()),
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    };

    let (_status, Json(release)) = routes_update_releases::upload_agent_update_artifact(
        State(state.clone()),
        HeaderMap::new(),
        Json(request),
    )
    .await
    .unwrap();

    assert_eq!(
        release.artifact_download_url.as_deref(),
        Some(
            format!("https://updates.example/api/v1/agent-update-artifacts/{sha256_hex}").as_str()
        )
    );
    assert_eq!(
        release.rollback_artifact_download_url.as_deref(),
        Some(
            format!("https://updates.example/api/v1/agent-update-artifacts/{rollback_sha256_hex}")
                .as_str()
        )
    );
    assert_eq!(
        release.rollback_artifact_sha256_hex.as_deref(),
        Some(rollback_sha256_hex.as_str())
    );
    assert_eq!(
        release.rollback_artifact_download_path.as_deref(),
        Some(format!("/api/v1/agent-update-artifacts/{rollback_sha256_hex}").as_str())
    );
    let latest = routes_update_releases::latest_agent_update_release(
        State(state.clone()),
        HeaderMap::new(),
        Query(routes_update_releases::LatestReleaseQuery {
            name: "vpsman-agent".to_string(),
            channel: "stable".to_string(),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(latest.version, "2.2.0");
    assert!(latest.rollback_artifact_download_url.is_some());

    let response = routes_update_releases::download_agent_update_artifact(
        State(state),
        Path(rollback_sha256_hex),
    )
    .await
    .unwrap();
    let downloaded = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(downloaded.as_ref(), rollback_artifact.as_slice());
}

#[tokio::test]
async fn streamed_artifacts_can_record_hosted_release_with_rollback() {
    let repo = Repository::Memory(MemoryState::default());
    let signing_key = SigningKey::from_bytes(&[55_u8; 32]);
    let rollback_key = SigningKey::from_bytes(&[56_u8; 32]);
    let artifact = b"streamed-update-artifact".to_vec();
    let rollback_artifact = b"streamed-rollback-artifact".to_vec();
    let sha256_hex = payload_hash(&artifact);
    let rollback_sha256_hex = payload_hash(&rollback_artifact);
    let store_root = std::env::temp_dir().join(format!("vpsman-update-stream-{}", Uuid::new_v4()));
    let state = AppState {
        repo: repo.clone(),
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: Some(BackupObjectStore::filesystem(store_root).unwrap()),
        update_artifact_public_base_url: Some("https://updates.example".to_string()),
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    };
    let signature_hex = hex::encode(sign_update_artifact_hash(&signing_key, &sha256_hex));
    let signing_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
    let rollback_signature_hex = hex::encode(sign_update_artifact_hash(
        &rollback_key,
        &rollback_sha256_hex,
    ));
    let rollback_signing_key_hex = hex::encode(rollback_key.verifying_key().to_bytes());

    let mut headers = HeaderMap::new();
    headers.insert(
        "x-vpsman-artifact-signature-hex",
        signature_hex.parse().unwrap(),
    );
    headers.insert(
        "x-vpsman-artifact-signing-key-hex",
        signing_key_hex.parse().unwrap(),
    );
    headers.insert("x-vpsman-confirmed", "true".parse().unwrap());
    let (status, Json(streamed)) = routes_update_releases::stream_agent_update_artifact(
        State(state.clone()),
        headers,
        Body::from(artifact.clone()),
    )
    .await
    .unwrap();
    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(streamed.artifact_sha256_hex, sha256_hex);
    assert_eq!(streamed.size_bytes, artifact.len() as i64);
    assert!(streamed.artifact_download_url.is_some());

    let mut rollback_headers = HeaderMap::new();
    rollback_headers.insert(
        "x-vpsman-artifact-signature-hex",
        rollback_signature_hex.parse().unwrap(),
    );
    rollback_headers.insert(
        "x-vpsman-artifact-signing-key-hex",
        rollback_signing_key_hex.parse().unwrap(),
    );
    rollback_headers.insert("x-vpsman-confirmed", "true".parse().unwrap());
    let (_status, Json(rollback_streamed)) = routes_update_releases::stream_agent_update_artifact(
        State(state.clone()),
        rollback_headers,
        Body::from(rollback_artifact.clone()),
    )
    .await
    .unwrap();
    assert_eq!(rollback_streamed.artifact_sha256_hex, rollback_sha256_hex);

    let (status, Json(release)) = routes_update_releases::create_hosted_agent_update_release(
        State(state.clone()),
        HeaderMap::new(),
        Json(CreateHostedAgentUpdateReleaseRequest {
            name: "vpsman-agent".to_string(),
            version: "2.3.0".to_string(),
            channel: "stable".to_string(),
            artifact_sha256_hex: sha256_hex.clone(),
            artifact_signature_hex: signature_hex,
            artifact_signing_key_hex: signing_key_hex.clone(),
            rollback_artifact_sha256_hex: Some(rollback_sha256_hex.clone()),
            rollback_artifact_signature_hex: Some(rollback_signature_hex),
            rollback_artifact_signing_key_hex: Some(rollback_signing_key_hex),
            notes: Some("streamed hosted release".to_string()),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(release.status, "artifact_hosted");
    assert_eq!(release.artifact_sha256_hex, sha256_hex);
    assert_eq!(
        release.rollback_artifact_sha256_hex.as_deref(),
        Some(rollback_sha256_hex.as_str())
    );
    assert!(release.artifact_download_url.is_some());
    assert!(release.rollback_artifact_download_url.is_some());

    let response = routes_update_releases::download_agent_update_artifact(
        State(state),
        Path(rollback_sha256_hex),
    )
    .await
    .unwrap();
    let downloaded = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(downloaded.as_ref(), rollback_artifact.as_slice());
    let audits = repo.list_audit_logs(10).await.unwrap();
    assert!(audits
        .iter()
        .any(|audit| audit.action == "agent_update.artifact_streamed"));
    let upload_audit = audits
        .iter()
        .find(|audit| audit.action == "agent_update.artifact_uploaded")
        .unwrap();
    assert_eq!(
        upload_audit.metadata["artifact_ingestion_mode"],
        "streamed_hosted_reference"
    );
    let audit_json = serde_json::to_string(&upload_audit.metadata).unwrap();
    assert!(!audit_json.contains("streamed-update-artifact"));
    assert!(!audit_json.contains(&signing_key_hex));
}

#[tokio::test]
async fn release_policy_rejects_disallowed_channels_and_untrusted_keys() {
    let repo = Repository::Memory(MemoryState::default());
    let trusted_key = SigningKey::from_bytes(&[61_u8; 32]);
    let untrusted_key = SigningKey::from_bytes(&[62_u8; 32]);
    let mut state = AppState {
        repo,
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: UpdateReleasePolicy::new(
            vec!["stable".to_string()],
            vec![hex::encode(trusted_key.verifying_key().to_bytes())],
        )
        .unwrap(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    };
    let mut request = signed_release_request("vpsman-agent", "2.4.0", "nightly");
    request.artifact_signing_key_hex = hex::encode(trusted_key.verifying_key().to_bytes());
    request.artifact_signature_hex = hex::encode(sign_update_artifact_hash(
        &trusted_key,
        &request.artifact_sha256_hex,
    ));
    let error = routes_update_releases::create_agent_update_release(
        State(state.clone()),
        HeaderMap::new(),
        Json(request),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "agent_update_release_channel_not_allowed");

    state.update_object_store = None;
    let mut request = signed_release_request("vpsman-agent", "2.4.1", "stable");
    request.artifact_signing_key_hex = hex::encode(untrusted_key.verifying_key().to_bytes());
    request.artifact_signature_hex = hex::encode(sign_update_artifact_hash(
        &untrusted_key,
        &request.artifact_sha256_hex,
    ));
    let error = routes_update_releases::create_agent_update_release(
        State(state),
        HeaderMap::new(),
        Json(request),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "agent_update_release_signing_key_untrusted");
}

#[test]
fn update_object_store_builds_explicit_s3_store_and_rejects_partial_config() {
    let mut args = test_args();
    args.update_object_endpoint = Some("http://127.0.0.1:9000".to_string());
    args.update_object_bucket = Some("vpsman-updates".to_string());
    args.update_object_access_key = Some("access".to_string());
    args.update_object_secret_key = Some("secret".to_string());
    args.update_object_create_bucket = true;

    let store = build_update_object_store(&args).unwrap().unwrap();
    assert_eq!(store.kind(), "s3");

    let mut partial = test_args();
    partial.update_object_endpoint = Some("http://127.0.0.1:9000".to_string());
    let error = build_update_object_store(&partial).unwrap_err().to_string();
    assert!(error.contains("VPSMAN_UPDATE_OBJECT_ENDPOINT"));
    assert!(error.contains("VPSMAN_UPDATE_OBJECT_BUCKET"));
}

#[test]
fn update_object_store_uses_filesystem_fallback_when_only_directory_is_configured() {
    let mut args = test_args();
    args.update_object_store_dir =
        Some(std::env::temp_dir().join(format!("vpsman-update-fs-{}", Uuid::new_v4())));

    let store = build_update_object_store(&args).unwrap().unwrap();
    assert_eq!(store.kind(), "filesystem");
}

#[test]
fn object_store_prefers_filesystem_directory_over_explicit_s3_config() {
    let mut args = test_args();
    args.backup_object_store_dir =
        Some(std::env::temp_dir().join(format!("vpsman-backup-fs-{}", Uuid::new_v4())));
    args.update_object_store_dir =
        Some(std::env::temp_dir().join(format!("vpsman-update-fs-{}", Uuid::new_v4())));
    args.object_endpoint = Some("http://127.0.0.1:9000".to_string());
    args.object_bucket = Some("vpsman-backups".to_string());
    args.object_access_key = Some("access".to_string());
    args.object_secret_key = Some("secret".to_string());
    args.update_object_endpoint = Some("http://127.0.0.1:9000".to_string());
    args.update_object_bucket = Some("vpsman-updates".to_string());
    args.update_object_access_key = Some("access".to_string());
    args.update_object_secret_key = Some("secret".to_string());

    let backup_store = build_backup_object_store(&args).unwrap().unwrap();
    let update_store = build_update_object_store(&args).unwrap().unwrap();

    assert_eq!(backup_store.kind(), "filesystem");
    assert_eq!(update_store.kind(), "filesystem");
}

fn test_operator() -> AuthContext {
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

fn signed_release_request(
    name: &str,
    version: &str,
    channel: &str,
) -> CreateAgentUpdateReleaseRequest {
    let signing_key = SigningKey::from_bytes(&[42_u8; 32]);
    let sha256_hex = "12".repeat(32);
    CreateAgentUpdateReleaseRequest {
        name: name.to_string(),
        version: version.to_string(),
        channel: channel.to_string(),
        artifact_sha256_hex: sha256_hex.clone(),
        artifact_signature_hex: hex::encode(sign_update_artifact_hash(&signing_key, &sha256_hex)),
        artifact_signing_key_hex: hex::encode(signing_key.verifying_key().to_bytes()),
        artifact_url: Some("https://updates.example/vpsman-agent".to_string()),
        rollback_artifact_sha256_hex: None,
        rollback_artifact_signature_hex: None,
        rollback_artifact_signing_key_hex: None,
        rollback_artifact_url: None,
        rollback_size_bytes: None,
        size_bytes: Some(1024),
        notes: Some("staging candidate".to_string()),
        confirmed: true,
    }
}

fn test_args() -> Args {
    Args {
        bind: "127.0.0.1:0".parse().unwrap(),
        postgres_url: None,
        debug_internal_test_mode: false,
        migrations_dir: PathBuf::from("migrations"),
        internal_token: None,
        gateway_control_url: None,
        server_signing_key_hex: None,
        discovery_trusted_server_public_keys_hex: Vec::new(),
        public_gateway_endpoints: Vec::new(),
        discovery_url: None,
        gateway_server_public_key_hex: None,
        enrollment_telemetry_light_secs: 15,
        enrollment_telemetry_full_secs: 60,
        enrollment_default_country: "US".to_string(),
        enrollment_unmanaged_update_enabled: true,
        enrollment_unmanaged_update_version_url: None,
        enrollment_unmanaged_update_interval_secs: 86_400,
        enrollment_unmanaged_update_jitter_secs: 86_400,
        enrollment_unmanaged_update_activate: true,
        enrollment_unmanaged_update_restart_agent: true,
        backup_object_store_dir: None,
        update_object_store_dir: None,
        update_object_endpoint: None,
        update_object_bucket: None,
        update_object_access_key: None,
        update_object_secret_key: None,
        update_object_region: "us-east-1".to_string(),
        update_object_create_bucket: false,
        update_artifact_public_base_url: None,
        agent_update_allowed_channels: Vec::new(),
        agent_update_trusted_signing_keys_hex: Vec::new(),
        object_endpoint: None,
        object_bucket: None,
        object_access_key: None,
        object_secret_key: None,
        object_region: "us-east-1".to_string(),
        object_create_bucket: false,
        job_output_artifact_min_bytes: 32768,
        agent_update_heartbeat_timeout_secs: DEFAULT_AGENT_UPDATE_HEARTBEAT_TIMEOUT_SECS as u64,
        agent_update_reconcile_interval_secs: 30,
        require_registered_agent_updates: false,
        alert_memory_available_warning_ratio: 0.20,
        alert_memory_available_critical_ratio: 0.10,
        alert_disk_available_warning_ratio: 0.20,
        alert_disk_available_critical_ratio: 0.10,
        alert_cpu_load_warning: 2.0,
        alert_cpu_load_critical: 4.0,
    }
}
