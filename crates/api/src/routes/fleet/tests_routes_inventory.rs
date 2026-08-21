use super::{
    peer_client_ids_for_deleted_agent, telemetry_network_rate_limit_or_default,
    validate_legacy_tag_name_for_cleanup, validate_persisted_tag_name,
    validate_suspend_agent_status, validate_telemetry_network_rate_query,
    validate_telemetry_rollup_query,
};
use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        AgentView, JobHistoryView, JobTargetView, OperatorPreferences, OperatorRecord,
        TelemetryNetworkRateQuery, TelemetryRollupQuery,
    },
    repository::{MemoryState, Repository},
    security::SCOPE_FLEET_READ,
    state::{AppState, DispatcherRuntimeConfig},
};
use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, header::CONTENT_TYPE, Request, StatusCode},
};
use tower::ServiceExt;
use uuid::Uuid;

#[test]
fn persisted_tags_reject_inner_selector_prefixes() {
    validate_persisted_tag_name("provider:alpha").unwrap();
    validate_persisted_tag_name("country:US").unwrap();
    validate_persisted_tag_name("region:legacy-name").unwrap();

    assert!(validate_persisted_tag_name("id:edge-a").is_err());
    assert!(validate_persisted_tag_name("name:edge-a").is_err());
    assert!(validate_persisted_tag_name("provider:").is_err());
    assert!(validate_persisted_tag_name(":alpha").is_err());
    assert!(validate_persisted_tag_name("role::edge").is_err());
    validate_legacy_tag_name_for_cleanup("provider:").unwrap();
    validate_legacy_tag_name_for_cleanup(":alpha").unwrap();
    validate_legacy_tag_name_for_cleanup("role::edge").unwrap();
}

#[tokio::test]
async fn legacy_runtime_config_patch_route_is_removed() {
    let (state, _) = tag_order_route_test_state();
    let response = crate::routes::build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/runtime-config/patch")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn telemetry_network_rates_allow_fleet_scale_limits() {
    assert_eq!(telemetry_network_rate_limit_or_default(None), 100);
    assert_eq!(telemetry_network_rate_limit_or_default(Some(5_000)), 5_000);
    assert_eq!(telemetry_network_rate_limit_or_default(Some(50_000)), 5_000);
}

#[test]
fn telemetry_queries_accept_adaptive_minute_aligned_spans() {
    for bucket_secs in [60, 120, 300, 86_400] {
        validate_telemetry_rollup_query(&TelemetryRollupQuery {
            limit: None,
            client_id: None,
            bucket_secs: Some(bucket_secs),
            latest: false,
        })
        .unwrap();
        validate_telemetry_network_rate_query(&TelemetryNetworkRateQuery {
            limit: None,
            client_id: None,
            interface: None,
            bucket_secs: Some(bucket_secs),
            latest: false,
        })
        .unwrap();
    }

    for bucket_secs in [-60, 0, 59, 61] {
        assert!(validate_telemetry_rollup_query(&TelemetryRollupQuery {
            limit: None,
            client_id: None,
            bucket_secs: Some(bucket_secs),
            latest: false,
        })
        .is_err());
        assert!(
            validate_telemetry_network_rate_query(&TelemetryNetworkRateQuery {
                limit: None,
                client_id: None,
                interface: None,
                bucket_secs: Some(bucket_secs),
                latest: false,
            })
            .is_err()
        );
    }
}

#[test]
fn deleting_agent_collects_each_declared_tunnel_peer_once() {
    let peers = peer_client_ids_for_deleted_agent(
        "edge-a",
        [
            ("edge-a".to_string(), "edge-b".to_string()),
            ("edge-c".to_string(), "edge-a".to_string()),
            ("edge-a".to_string(), "edge-b".to_string()),
            ("edge-c".to_string(), "edge-d".to_string()),
        ],
    );
    assert_eq!(peers.into_iter().collect::<Vec<_>>(), ["edge-b", "edge-c"]);
}

#[test]
fn suspension_status_eligibility_is_exact() {
    for status in ["never", "disconnected", "offline", "stale"] {
        validate_suspend_agent_status(status).unwrap();
    }
    for status in ["online", "suspended", "revoked", "deleted"] {
        assert!(validate_suspend_agent_status(status).is_err(), "{status}");
    }
}

#[tokio::test]
async fn suspension_routes_skip_unstarted_targets_and_restore_never_status() {
    let (state, memory) = tag_order_route_test_state();
    memory.agents.write().await.push(AgentView {
        id: "never-suspend".to_string(),
        display_name: "Never Suspend".to_string(),
        status: "never".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: Default::default(),
    });
    memory.agents.write().await.push(AgentView {
        id: "online-rejected".to_string(),
        display_name: "Online Rejected".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some(crate::unix_now().to_string()),
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: Some(Uuid::new_v4()),
        stale_since: None,
        stale_reason: None,
        capabilities: Default::default(),
    });
    let job_id = Uuid::new_v4();
    memory.jobs.write().await.push(JobHistoryView {
        id: job_id,
        actor_id: None,
        command_type: "run_script".to_string(),
        source_schedule_id: None,
        causation_id: None,
        schedule_lineage: Vec::new(),
        privileged: false,
        status: "queued".to_string(),
        target_count: 1,
        payload_hash: "suspension-test".to_string(),
        max_timeout_secs: 60,
        created_at: crate::unix_now().to_string(),
        completed_at: None,
    });
    memory.job_targets.write().await.push(JobTargetView {
        job_id,
        client_id: "never-suspend".to_string(),
        status: "queued".to_string(),
        message: None,
        exit_code: None,
        started_at: None,
        deadline_at: None,
        completed_at: None,
        process_incarnation_id: None,
    });
    let token = issue_tag_order_token(&state, &memory, "operator", &["inventory:write"]).await;
    let router = crate::routes::build_router(state);

    let unconfirmed = router
        .clone()
        .oneshot(agent_suspension_request(
            "/api/v1/agents/never-suspend/suspend",
            &token,
            serde_json::json!({"confirmed": false}),
        ))
        .await
        .unwrap();
    assert_eq!(unconfirmed.status(), StatusCode::CONFLICT);

    let suspended = router
        .clone()
        .oneshot(agent_suspension_request(
            "/api/v1/agents/never-suspend/suspend",
            &token,
            serde_json::json!({"confirmed": true, "reason": "planned maintenance"}),
        ))
        .await
        .unwrap();
    assert_eq!(suspended.status(), StatusCode::OK);
    let suspended_json: serde_json::Value =
        serde_json::from_slice(&to_bytes(suspended.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(suspended_json["agent"]["status"], "suspended");
    assert_eq!(suspended_json["suspended_from_status"], "never");
    assert_eq!(
        suspended_json["skipped_unstarted_job_ids"],
        serde_json::json!([job_id])
    );
    let target = memory.job_targets.read().await[0].clone();
    assert_eq!(target.status, "skipped");
    assert_eq!(
        target.message.as_deref(),
        Some("target_suspended: target skipped because VPS is suspended")
    );

    let online = router
        .clone()
        .oneshot(agent_suspension_request(
            "/api/v1/agents/online-rejected/suspend",
            &token,
            serde_json::json!({"confirmed": true}),
        ))
        .await
        .unwrap();
    assert_eq!(online.status(), StatusCode::CONFLICT);

    let unsuspended = router
        .oneshot(agent_suspension_request(
            "/api/v1/agents/never-suspend/unsuspend",
            &token,
            serde_json::json!({"confirmed": true}),
        ))
        .await
        .unwrap();
    assert_eq!(unsuspended.status(), StatusCode::OK);
    let unsuspended_json: serde_json::Value =
        serde_json::from_slice(&to_bytes(unsuspended.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(unsuspended_json["agent"]["status"], "never");
    assert!(memory.agent_suspensions.read().await.is_empty());
}

#[tokio::test]
async fn tag_order_routes_enforce_authority_and_exact_json_contracts() {
    let (state, memory) = tag_order_route_test_state();
    state
        .repo
        .create_tag_name("provider:A10".to_string())
        .await
        .unwrap();
    state
        .repo
        .create_tag_name("provider:A2".to_string())
        .await
        .unwrap();
    let fleet_reader = issue_tag_order_token(&state, &memory, "viewer", &[SCOPE_FLEET_READ]).await;
    let inventory_operator =
        issue_tag_order_token(&state, &memory, "operator", &["inventory:write"]).await;
    let write_scoped_viewer =
        issue_tag_order_token(&state, &memory, "viewer", &["inventory:write"]).await;
    let read_only_operator =
        issue_tag_order_token(&state, &memory, "operator", &[SCOPE_FLEET_READ]).await;
    let router = crate::routes::build_router(state.clone());

    let get_response = router
        .clone()
        .oneshot(tag_order_request("GET", &fleet_reader, None))
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json: serde_json::Value = serde_json::from_slice(
        &to_bytes(get_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        get_json,
        serde_json::json!({
            "tags": [
                {"name": "provider:A10", "display_order": 1024, "clients": []},
                {"name": "provider:A2", "display_order": 2048, "clients": []}
            ],
            "namespace_natural_sort_enabled": false
        })
    );

    let denied_get = router
        .clone()
        .oneshot(tag_order_request("GET", &inventory_operator, None))
        .await
        .unwrap();
    assert_eq!(denied_get.status(), StatusCode::FORBIDDEN);

    let put_body = serde_json::json!({
        "ordered_tags": ["provider:A10", "provider:A2"],
        "namespace_natural_sort_enabled": true
    });
    let put_response = router
        .clone()
        .oneshot(tag_order_request(
            "PUT",
            &inventory_operator,
            Some(put_body.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(put_response.status(), StatusCode::OK);
    let put_json: serde_json::Value = serde_json::from_slice(
        &to_bytes(put_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        put_json,
        serde_json::json!({
            "tags": [
                {"name": "provider:A2", "display_order": 1024, "clients": []},
                {"name": "provider:A10", "display_order": 2048, "clients": []}
            ],
            "namespace_natural_sort_enabled": true
        })
    );

    for denied_token in [&write_scoped_viewer, &read_only_operator] {
        let response = router
            .clone()
            .oneshot(tag_order_request(
                "PUT",
                denied_token,
                Some(put_body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    for malformed in [
        serde_json::json!({"ordered_tags": ["provider:A2", "provider:A10"]}),
        serde_json::json!({
            "ordered_tags": ["provider:A2", "provider:A10"],
            "namespace_natural_sort_enabled": true,
            "natural_sort": true
        }),
    ] {
        let response = router
            .clone()
            .oneshot(tag_order_request(
                "PUT",
                &inventory_operator,
                Some(malformed),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}

fn tag_order_route_test_state() -> (AppState, MemoryState) {
    let memory = MemoryState::default();
    let (events, _) = crate::state::WsEventBus::new(1);
    (
        AppState {
            repo: Repository::Memory(memory.clone()),
            events,
            internal_token: None,
            gateway: GatewayDispatchClient::default(),
            backup_object_store: None,
            update_release_policy: Default::default(),
            job_output_artifact_min_bytes: 32_768,
            artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
            require_registered_agent_updates: false,
            suite_config_path: "config/vpsman.toml".into(),
            dispatcher_config: DispatcherRuntimeConfig::default(),
        },
        memory,
    )
}

async fn issue_tag_order_token(
    state: &AppState,
    memory: &MemoryState,
    role: &str,
    scopes: &[&str],
) -> String {
    let operator = OperatorRecord {
        id: Uuid::new_v4(),
        username: format!("tag-order-{role}-{}", Uuid::new_v4()),
        password_hash: "test-only-session-issued-directly".to_string(),
        role: role.to_string(),
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        preferences: OperatorPreferences::default(),
        totp_enabled: false,
        totp_secret_ciphertext_hex: None,
        totp_secret_nonce_hex: None,
        totp_secret_salt_hex: None,
        totp_last_accepted_step: None,
        status: "active".to_string(),
        session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
        created_at: crate::unix_now().to_string(),
        disabled_at: None,
        deleted_at: None,
    };
    memory.operators.write().await.push(operator.clone());
    state
        .repo
        .issue_session(operator.view())
        .await
        .unwrap()
        .access_token
}

fn tag_order_request(method: &str, token: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri("/api/v1/tags/order")
        .header(AUTHORIZATION, format!("Bearer {token}"));
    if body.is_some() {
        request = request.header(CONTENT_TYPE, "application/json");
    }
    request
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap()
}

fn agent_suspension_request(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}
