use super::*;
use std::collections::BTreeMap;

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, HeaderMap, Request, StatusCode},
};
use tower::ServiceExt;

use crate::model_command_templates::{CommandTemplateQuery, JobOutputComparisonQuery};
use crate::security::{
    default_operator_scopes, SCOPE_AUDIT_READ, SCOPE_BACKUPS_READ, SCOPE_CONFIG_READ,
    SCOPE_FLEET_READ, SCOPE_HISTORY_WRITE, SCOPE_INTEGRATIONS_READ, SCOPE_INTEGRATIONS_WRITE,
    SCOPE_JOBS_READ, SCOPE_NETWORK_READ, SCOPE_SCHEDULES_READ, SCOPE_TEMPLATES_READ,
    SCOPE_TEMPLATES_WRITE, SCOPE_TERMINAL_READ,
};

#[test]
fn operator_password_hash_verifies_without_plaintext_storage() {
    let hash = hash_operator_password("correct horse battery staple").unwrap();

    assert!(hash.starts_with("argon2id$v=19$"));
    assert!(!hash.contains("correct horse battery staple"));
    assert!(verify_operator_password("correct horse battery staple", &hash).unwrap());
    assert!(!verify_operator_password("wrong horse battery staple", &hash).unwrap());
}

#[test]
fn generated_operator_tokens_are_hashed_for_storage() {
    let token = generate_token();
    let hash = token_hash(&token);

    assert_eq!(token.len(), 64);
    assert_eq!(hash.len(), 64);
    assert_ne!(token, hash);
    assert_eq!(token_hash(&token), hash);
}

#[tokio::test]
async fn bootstrap_operator_rejects_second_admin_in_repository() {
    let repo = Repository::Memory(MemoryState::default());

    repo.bootstrap_operator(&BootstrapOperatorRequest {
        username: "admin".to_string(),
        password: "admin-password-123".to_string(),
    })
    .await
    .unwrap();
    let error = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "other-admin".to_string(),
            password: "other-admin-password-123".to_string(),
        })
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "operator_already_bootstrapped");
    assert_eq!(repo.operator_count().await.unwrap(), 1);
}

#[tokio::test]
async fn concurrent_bootstrap_operator_creates_exactly_one_admin() {
    let repo = Repository::Memory(MemoryState::default());
    let mut tasks = Vec::new();

    for index in 0..16 {
        let repo = repo.clone();
        tasks.push(tokio::spawn(async move {
            repo.bootstrap_operator(&BootstrapOperatorRequest {
                username: format!("admin-{index}"),
                password: "admin-password-123".to_string(),
            })
            .await
            .map(|auth| auth.operator.username)
        }));
    }

    let mut created = Vec::new();
    let mut rejected = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(username) => created.push(username),
            Err(error) if error.to_string() == "operator_already_bootstrapped" => rejected += 1,
            Err(error) => panic!("unexpected bootstrap error: {error}"),
        }
    }

    assert_eq!(created.len(), 1);
    assert_eq!(rejected, 15);
    assert_eq!(repo.operator_count().await.unwrap(), 1);
}

#[tokio::test]
async fn bootstrap_status_route_reports_first_operator_requirement() {
    let state = memory_test_state();

    let response = crate::routes::build_router(state.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/auth/bootstrap-status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload, serde_json::json!({ "bootstrap_required": true }));

    state
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();

    let response = crate::routes::build_router(state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/auth/bootstrap-status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload, serde_json::json!({ "bootstrap_required": false }));
}

#[tokio::test]
async fn bootstrap_route_links_initial_session_to_request_origin() {
    let state = memory_test_state();
    let peer = "203.0.113.39:44321"
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let response = crate::routes::build_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/bootstrap")
                .header("content-type", "application/json")
                .header("user-agent", "bootstrap-audit-browser")
                .extension(axum::extract::ConnectInfo(peer))
                .body(Body::from(
                    serde_json::json!({
                        "username": "admin",
                        "password": "admin-password-123"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session_id = payload["session_id"].as_str().unwrap();
    let event = state
        .repo
        .list_operator_auth_events(&OperatorAuthEventQuery {
            limit: None,
            operator_id: None,
            username: None,
            result: None,
        })
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.session_id.map(|id| id.to_string()) == Some(session_id.to_string()))
        .expect("bootstrap session auth event");

    assert_eq!(event.result, "success");
    assert_eq!(event.remote_ip.as_deref(), Some("203.0.113.39"));
    assert_eq!(event.user_agent.as_deref(), Some("bootstrap-audit-browser"));
}

#[tokio::test]
async fn refresh_operator_session_rotates_refresh_token_once() {
    let repo = Repository::Memory(MemoryState::default());
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();

    let replacement = repo
        .refresh_operator_session(&auth.refresh_token)
        .await
        .unwrap();
    assert!(replacement.is_some());
    let replay = repo
        .refresh_operator_session(&auth.refresh_token)
        .await
        .unwrap();

    assert!(replay.is_none());
}

#[tokio::test]
async fn successful_login_audit_links_the_issued_session_and_request_origin() {
    let repo = Repository::Memory(MemoryState::default());
    let password = "admin-password-123";
    repo.bootstrap_operator(&BootstrapOperatorRequest {
        username: "admin".to_string(),
        password: password.to_string(),
    })
    .await
    .unwrap();
    let attempt = repo
        .login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: password.to_string(),
                totp_code: None,
            },
            "203.0.113.40",
            Some("audit-test-agent"),
            &crate::state::OperatorAuthThrottleConfig::default(),
        )
        .await
        .unwrap();
    let repository_auth::OperatorLoginAttempt::Authenticated(response) = attempt else {
        panic!("expected authenticated login")
    };
    let audit = repo
        .list_audit_logs(20)
        .await
        .unwrap()
        .into_iter()
        .find(|audit| audit.action == "operator_auth.login_success")
        .expect("login audit");

    assert_eq!(
        audit.metadata["operator_session_id"],
        response.session_id.to_string()
    );
    assert_eq!(audit.metadata["remote_ip"], "203.0.113.40");
    assert_eq!(audit.metadata["user_agent"], "audit-test-agent");
}

#[tokio::test]
async fn operator_auth_event_listing_rejects_noncanonical_audit_rows() {
    let memory = MemoryState::default();
    memory.audits.write().await.push(AuditLogView {
        id: Uuid::new_v4(),
        actor_id: None,
        action: "operator_auth.login_failure".to_string(),
        target: "operator-login:test-operator".to_string(),
        command_hash: None,
        metadata: serde_json::json!({
            "attempted_username": "test-operator",
            "component": "operator-auth",
            "origin_kind": "authentication",
            "result": "   "
        }),
        created_at: unix_now().to_string(),
    });
    let repo = Repository::Memory(memory);

    let error = repo
        .list_operator_auth_events(&OperatorAuthEventQuery {
            limit: None,
            operator_id: None,
            username: None,
            result: None,
        })
        .await
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("operator auth audit missing canonical result"));
}

#[tokio::test]
async fn operator_auth_event_listing_rejects_malformed_canonical_ids() {
    for malformed_session_id in [serde_json::json!(17), serde_json::json!("not-a-uuid")] {
        let memory = MemoryState::default();
        memory.audits.write().await.push(AuditLogView {
            id: Uuid::new_v4(),
            actor_id: None,
            action: "operator_auth.login_failure".to_string(),
            target: "auth:login".to_string(),
            command_hash: None,
            metadata: serde_json::json!({
                "attempted_username": "test-operator",
                "component": "operator-auth",
                "operator_session_id": malformed_session_id,
                "origin_kind": "authentication",
                "result": "failure"
            }),
            created_at: unix_now().to_string(),
        });
        let repo = Repository::Memory(memory);

        let error = repo
            .list_operator_auth_events(&OperatorAuthEventQuery {
                limit: None,
                operator_id: None,
                username: None,
                result: None,
            })
            .await
            .unwrap_err();

        assert!(error.to_string().contains("operator_session_id"));
    }
}

#[tokio::test]
async fn logout_route_revokes_current_session_idempotently_and_audits_once() {
    let state = memory_test_state();
    let auth = state
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();
    let app = crate::routes::build_router(state.clone());
    let logout_request = || {
        let peer = "203.0.113.41:44321"
            .parse::<std::net::SocketAddr>()
            .unwrap();
        Request::builder()
            .method("POST")
            .uri("/api/v1/auth/logout")
            .header(AUTHORIZATION, format!("Bearer {}", auth.access_token))
            .extension(axum::extract::ConnectInfo(peer))
            .body(Body::empty())
            .unwrap()
    };

    let response = app.clone().oneshot(logout_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(state
        .repo
        .authenticate_access_token(&auth.access_token)
        .await
        .unwrap()
        .is_none());
    assert!(state
        .repo
        .refresh_operator_session(&auth.refresh_token)
        .await
        .unwrap()
        .is_none());

    let retry = app.oneshot(logout_request()).await.unwrap();
    assert_eq!(retry.status(), StatusCode::NO_CONTENT);
    let logout_audits = state
        .repo
        .list_audit_logs(100)
        .await
        .unwrap()
        .into_iter()
        .filter(|audit| audit.action == "operator_session.logged_out")
        .collect::<Vec<_>>();
    assert_eq!(logout_audits.len(), 1);
    assert_eq!(
        logout_audits[0].metadata["revocation_scope"],
        "current_session"
    );
    assert_eq!(
        logout_audits[0].metadata["revoked_access_and_refresh"],
        true
    );
    assert_eq!(
        logout_audits[0].metadata["operator_session_id"],
        auth.session_id.to_string()
    );
    assert_eq!(logout_audits[0].metadata["result"], "succeeded");
    assert_eq!(logout_audits[0].metadata["remote_ip"], "203.0.113.41");
    let audit_json = serde_json::to_string(&logout_audits[0]).unwrap();
    assert!(!audit_json.contains(&auth.access_token));
    assert!(!audit_json.contains(&auth.refresh_token));
}

#[tokio::test]
async fn logout_route_rejects_missing_or_unknown_session_credentials() {
    let app = crate::routes::build_router(memory_test_state());
    let peer = "203.0.113.42:44321"
        .parse::<std::net::SocketAddr>()
        .unwrap();

    let missing = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .extension(axum::extract::ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let unknown = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(AUTHORIZATION, format!("Bearer {}", "f".repeat(64)))
                .extension(axum::extract::ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn concurrent_refresh_operator_session_mints_one_replacement() {
    let repo = Repository::Memory(MemoryState::default());
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();
    let mut tasks = Vec::new();

    for _ in 0..16 {
        let repo = repo.clone();
        let refresh_token = auth.refresh_token.clone();
        tasks.push(tokio::spawn(async move {
            repo.refresh_operator_session(&refresh_token).await
        }));
    }

    let mut replacements = 0;
    let mut rejected = 0;
    for task in tasks {
        match task.await.unwrap().unwrap() {
            Some(_) => replacements += 1,
            None => rejected += 1,
        }
    }

    assert_eq!(replacements, 1);
    assert_eq!(rejected, 15);
    assert!(repo
        .refresh_operator_session(&auth.refresh_token)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn operator_login_throttle_isolates_username_lockouts_by_client_ip() {
    let repo = Repository::Memory(MemoryState::default());
    let password = "admin-password-123";
    repo.bootstrap_operator(&BootstrapOperatorRequest {
        username: "admin".to_string(),
        password: password.to_string(),
    })
    .await
    .unwrap();
    let throttle = crate::state::OperatorAuthThrottleConfig {
        username_failed_attempt_limit: 2,
        ip_failed_attempt_limit: 100,
        failed_attempt_window_secs: 60,
        lockout_secs: 60,
    };

    assert!(matches!(
        repo.login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: "wrong-password-123".to_string(),
                totp_code: None,
            },
            "203.0.113.10",
            None,
            &throttle,
        )
        .await
        .unwrap(),
        repository_auth::OperatorLoginAttempt::InvalidCredentials
    ));
    assert!(matches!(
        repo.login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: password.to_string(),
                totp_code: None,
            },
            "203.0.113.10",
            None,
            &throttle,
        )
        .await
        .unwrap(),
        repository_auth::OperatorLoginAttempt::Authenticated(_)
    ));

    for _ in 0..2 {
        assert!(matches!(
            repo.login_operator_with_throttle(
                &LoginRequest {
                    username: "admin".to_string(),
                    password: "wrong-password-123".to_string(),
                    totp_code: None,
                },
                "203.0.113.10",
                None,
                &throttle,
            )
            .await
            .unwrap(),
            repository_auth::OperatorLoginAttempt::InvalidCredentials
        ));
    }
    assert!(matches!(
        repo.login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: password.to_string(),
                totp_code: None,
            },
            "203.0.113.10",
            None,
            &throttle,
        )
        .await
        .unwrap(),
        repository_auth::OperatorLoginAttempt::Throttled
    ));
    assert!(matches!(
        repo.login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: password.to_string(),
                totp_code: None,
            },
            "203.0.113.11",
            None,
            &throttle,
        )
        .await
        .unwrap(),
        repository_auth::OperatorLoginAttempt::Authenticated(_)
    ));
    assert!(matches!(
        repo.login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: password.to_string(),
                totp_code: None,
            },
            "203.0.113.10",
            None,
            &throttle,
        )
        .await
        .unwrap(),
        repository_auth::OperatorLoginAttempt::Throttled
    ));
    let audit_count_before = repo.list_audit_logs(100).await.unwrap().len();
    for _ in 0..3 {
        assert!(matches!(
            repo.login_operator_with_throttle(
                &LoginRequest {
                    username: "admin".to_string(),
                    password: "wrong-password-123".to_string(),
                    totp_code: None,
                },
                "203.0.113.10",
                None,
                &throttle,
            )
            .await
            .unwrap(),
            repository_auth::OperatorLoginAttempt::Throttled
        ));
    }
    assert_eq!(
        repo.list_audit_logs(100).await.unwrap().len(),
        audit_count_before
    );

    let audits = repo.list_audit_logs(10).await.unwrap();
    let lockout = audits
        .iter()
        .find(|audit| audit.action == "operator_auth.lockout_created")
        .expect("lockout audit");
    assert_eq!(lockout.target, "auth:login");
    assert_eq!(lockout.metadata["result"], "locked");
    assert_eq!(lockout.metadata["origin_kind"], "authentication");
    assert_eq!(lockout.metadata["component"], "operator-auth-throttle");
    assert_eq!(lockout.metadata["remote_ip"], "203.0.113.10");
    let audit_json = serde_json::to_string(&audits).unwrap();
    assert!(audit_json.contains("\"cleared_previous_failures\":true"));
    assert!(!audit_json.contains("operator_auth.login_after_failures"));
    assert!(audit_json.contains("operator_auth.lockout_created"));
    assert!(audit_json.contains("\"scope_kind\":\"username_ip\""));
    assert!(!audit_json.contains("\"scope_kind\":\"ip\""));
    assert!(!audit_json.contains("operator_auth.login_throttled"));
}

#[tokio::test]
async fn login_route_returns_too_many_requests_after_configured_failures() {
    let state = memory_test_state();
    let peer = "203.0.113.20:44321"
        .parse::<std::net::SocketAddr>()
        .unwrap();

    for _ in 0..8 {
        let error = routes_auth::login_operator(
            axum::extract::State(state.clone()),
            axum::extract::ConnectInfo(peer),
            HeaderMap::new(),
            axum::Json(LoginRequest {
                username: "missing-operator".to_string(),
                password: "valid-shaped-password-123".to_string(),
                totp_code: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "invalid_operator_credentials");
    }

    let error = routes_auth::login_operator(
        axum::extract::State(state),
        axum::extract::ConnectInfo(peer),
        HeaderMap::new(),
        axum::Json(LoginRequest {
            username: "missing-operator".to_string(),
            password: "valid-shaped-password-123".to_string(),
            totp_code: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error.code, "operator_login_throttled");
}

#[tokio::test]
async fn login_route_ip_throttle_spans_unknown_usernames() {
    let failure_limit = 8;
    let (state, suite_config_path) = memory_test_state_with_ip_throttle_limit(failure_limit);
    let peer = "203.0.113.21:44321"
        .parse::<std::net::SocketAddr>()
        .unwrap();

    for index in 0..failure_limit {
        let error = routes_auth::login_operator(
            axum::extract::State(state.clone()),
            axum::extract::ConnectInfo(peer),
            HeaderMap::new(),
            axum::Json(LoginRequest {
                username: format!("missing-operator-{index}"),
                password: "valid-shaped-password-123".to_string(),
                totp_code: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "invalid_operator_credentials");
    }

    let error = routes_auth::login_operator(
        axum::extract::State(state.clone()),
        axum::extract::ConnectInfo(peer),
        HeaderMap::new(),
        axum::Json(LoginRequest {
            username: "different-missing-operator".to_string(),
            password: "valid-shaped-password-123".to_string(),
            totp_code: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error.code, "operator_login_throttled");

    let other_peer = "203.0.113.22:44321"
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let error = routes_auth::login_operator(
        axum::extract::State(state),
        axum::extract::ConnectInfo(other_peer),
        HeaderMap::new(),
        axum::Json(LoginRequest {
            username: "different-missing-operator".to_string(),
            password: "valid-shaped-password-123".to_string(),
            totp_code: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::UNAUTHORIZED);
    assert_eq!(error.code, "invalid_operator_credentials");
    std::fs::remove_file(suite_config_path).unwrap();
}

#[tokio::test]
async fn login_route_throttles_by_forwarded_ipv6_operator_ip() {
    let failure_limit = 8;
    let (state, suite_config_path) = memory_test_state_with_ip_throttle_limit(failure_limit);
    let peer = "127.0.0.1:44321".parse::<std::net::SocketAddr>().unwrap();

    for index in 0..failure_limit {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "2001:db8::10".parse().unwrap());
        let error = routes_auth::login_operator(
            axum::extract::State(state.clone()),
            axum::extract::ConnectInfo(peer),
            headers,
            axum::Json(LoginRequest {
                username: format!("missing-operator-{index}"),
                password: "valid-shaped-password-123".to_string(),
                totp_code: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
    }

    let mut locked_headers = HeaderMap::new();
    locked_headers.insert("x-forwarded-for", "2001:db8::10".parse().unwrap());
    let error = routes_auth::login_operator(
        axum::extract::State(state.clone()),
        axum::extract::ConnectInfo(peer),
        locked_headers,
        axum::Json(LoginRequest {
            username: "different-missing-operator".to_string(),
            password: "valid-shaped-password-123".to_string(),
            totp_code: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);

    let mut other_headers = HeaderMap::new();
    other_headers.insert("x-forwarded-for", "2001:db8::11".parse().unwrap());
    let error = routes_auth::login_operator(
        axum::extract::State(state),
        axum::extract::ConnectInfo(peer),
        other_headers,
        axum::Json(LoginRequest {
            username: "different-missing-operator".to_string(),
            password: "valid-shaped-password-123".to_string(),
            totp_code: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::UNAUTHORIZED);
    std::fs::remove_file(suite_config_path).unwrap();
}

#[tokio::test]
async fn missing_totp_counts_toward_login_throttle() {
    let repo = Repository::Memory(MemoryState::default());
    let password = "admin-password-123";
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator,
        session_id: Some(Uuid::new_v4()),
    };
    let TotpSetupOutcome::Created(setup) =
        repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected TOTP setup");
    };
    let encrypted = repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .encrypted_totp_secret()
        .expect("encrypted totp secret");
    let secret = crate::auth_totp::decrypt_totp_secret(password, &encrypted).unwrap();
    let code = crate::auth_totp::totp_code_for_step(&secret, unix_now() / 30);
    let TotpUpdateOutcome::Updated(_) = repo
        .confirm_operator_totp(&actor, password, &code)
        .await
        .unwrap()
    else {
        panic!("expected TOTP enabled");
    };
    assert!(!setup.secret_base32.is_empty());

    let throttle = crate::state::OperatorAuthThrottleConfig {
        username_failed_attempt_limit: 1,
        ip_failed_attempt_limit: 100,
        failed_attempt_window_secs: 60,
        lockout_secs: 60,
    };
    assert!(matches!(
        repo.login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: password.to_string(),
                totp_code: None,
            },
            "203.0.113.23",
            None,
            &throttle,
        )
        .await
        .unwrap(),
        repository_auth::OperatorLoginAttempt::InvalidCredentials
    ));
    assert!(matches!(
        repo.login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: password.to_string(),
                totp_code: Some(code),
            },
            "203.0.113.23",
            None,
            &throttle,
        )
        .await
        .unwrap(),
        repository_auth::OperatorLoginAttempt::Throttled
    ));
}

#[tokio::test]
async fn totp_management_failures_use_operator_auth_throttle() {
    let state = memory_test_state();
    let password = "admin-password-123";
    let auth = state
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        format!("Bearer {}", auth.access_token).parse().unwrap(),
    );
    let peer = "203.0.113.40:44321"
        .parse::<std::net::SocketAddr>()
        .unwrap();

    for _ in 0..8 {
        let error = routes_auth::setup_operator_totp(
            axum::extract::State(state.clone()),
            axum::extract::ConnectInfo(peer),
            headers.clone(),
            axum::Json(TotpSetupRequest {
                password: "wrong-password-123".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "invalid_totp_credentials");
    }

    let error = routes_auth::setup_operator_totp(
        axum::extract::State(state),
        axum::extract::ConnectInfo(peer),
        headers,
        axum::Json(TotpSetupRequest {
            password: password.to_string(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error.code, "operator_auth_throttled");
}

#[tokio::test]
async fn totp_management_bad_credentials_preserve_session_and_factor_state() {
    let state = memory_test_state();
    let password = "admin-password-123";
    let auth = state
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        format!("Bearer {}", auth.access_token).parse().unwrap(),
    );
    let peer = "203.0.113.41:44321"
        .parse::<std::net::SocketAddr>()
        .unwrap();

    let bearer_error = routes_auth::setup_operator_totp(
        axum::extract::State(state.clone()),
        axum::extract::ConnectInfo(peer),
        HeaderMap::new(),
        axum::Json(TotpSetupRequest {
            password: password.to_string(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(bearer_error.status, StatusCode::UNAUTHORIZED);

    let setup_error = routes_auth::setup_operator_totp(
        axum::extract::State(state.clone()),
        axum::extract::ConnectInfo(peer),
        headers.clone(),
        axum::Json(TotpSetupRequest {
            password: "wrong-password-123".to_string(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(setup_error.status, StatusCode::BAD_REQUEST);
    assert_eq!(setup_error.code, "invalid_totp_credentials");
    assert_eq!(
        setup_error.public_message.as_deref(),
        Some("The current password is incorrect.")
    );
    let axum::Json(current) =
        routes_auth::current_operator(axum::extract::State(state.clone()), headers.clone())
            .await
            .unwrap();
    assert!(!current.totp_enabled);

    let _ = routes_auth::setup_operator_totp(
        axum::extract::State(state.clone()),
        axum::extract::ConnectInfo(peer),
        headers.clone(),
        axum::Json(TotpSetupRequest {
            password: password.to_string(),
        }),
    )
    .await
    .unwrap();
    let pending_before = state
        .repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap();
    let encrypted_before = pending_before
        .encrypted_totp_secret()
        .expect("pending encrypted TOTP secret");
    let secret = crate::auth_totp::decrypt_totp_secret(password, &encrypted_before).unwrap();
    let wrong_code = ["000000", "111111", "222222", "333333"]
        .into_iter()
        .find(|candidate| !crate::auth_totp::verify_totp_code(&secret, candidate, unix_now()))
        .expect("at least one candidate is outside the three-code TOTP window")
        .to_string();

    let confirm_error = routes_auth::confirm_operator_totp(
        axum::extract::State(state.clone()),
        axum::extract::ConnectInfo(peer),
        headers.clone(),
        axum::Json(TotpConfirmRequest {
            password: password.to_string(),
            code: wrong_code.clone(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(confirm_error.status, StatusCode::BAD_REQUEST);
    assert_eq!(confirm_error.code, "invalid_totp_credentials");
    assert_eq!(
        confirm_error.public_message.as_deref(),
        Some("The current password or authenticator code is incorrect.")
    );
    let pending_after = state
        .repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap();
    assert!(!pending_after.totp_enabled);
    let encrypted_after = pending_after
        .encrypted_totp_secret()
        .expect("failed confirmation preserves pending TOTP secret");
    assert_eq!(
        encrypted_after.ciphertext_hex,
        encrypted_before.ciphertext_hex
    );

    let code = crate::auth_totp::totp_code_for_step(&secret, unix_now() / 30);
    let axum::Json(enabled) = routes_auth::confirm_operator_totp(
        axum::extract::State(state.clone()),
        axum::extract::ConnectInfo(peer),
        headers.clone(),
        axum::Json(TotpConfirmRequest {
            password: password.to_string(),
            code,
        }),
    )
    .await
    .unwrap();
    assert!(enabled.totp_enabled);

    let disable_error = routes_auth::disable_operator_totp(
        axum::extract::State(state.clone()),
        axum::extract::ConnectInfo(peer),
        headers.clone(),
        axum::Json(TotpDisableRequest {
            password: password.to_string(),
            code: wrong_code,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(disable_error.status, StatusCode::BAD_REQUEST);
    assert_eq!(disable_error.code, "invalid_totp_credentials");
    assert_eq!(
        disable_error.public_message.as_deref(),
        Some("The current password or authenticator code is incorrect.")
    );
    let enabled_after = state
        .repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap();
    assert!(enabled_after.totp_enabled);
    assert_eq!(
        enabled_after
            .encrypted_totp_secret()
            .expect("failed disable preserves encrypted TOTP secret")
            .ciphertext_hex,
        encrypted_before.ciphertext_hex
    );

    let axum::Json(current) = routes_auth::current_operator(axum::extract::State(state), headers)
        .await
        .unwrap();
    assert!(current.totp_enabled);
}

#[test]
fn operator_roles_are_ranked_for_authorization() {
    assert!(role_allows("admin", "operator"));
    assert!(role_allows("operator", "viewer"));
    assert!(role_allows("viewer", "viewer"));
    assert!(!role_allows("viewer", "operator"));
    assert!(!role_allows("operator", "admin"));
    assert!(validate_operator_role("admin").is_ok());
    assert!(validate_operator_role("operator").is_ok());
    assert!(validate_operator_role("viewer").is_ok());
    assert_eq!(
        validate_operator_role("root").unwrap_err().code,
        "invalid_operator_role"
    );
}

#[test]
fn default_operator_scopes_keep_viewers_out_of_sensitive_reads() {
    let operator_scopes = default_operator_scopes("operator");
    for expected in [
        SCOPE_FLEET_READ,
        SCOPE_JOBS_READ,
        SCOPE_BACKUPS_READ,
        SCOPE_TERMINAL_READ,
        SCOPE_INTEGRATIONS_READ,
        SCOPE_TEMPLATES_READ,
        SCOPE_SCHEDULES_READ,
        SCOPE_CONFIG_READ,
        SCOPE_NETWORK_READ,
        SCOPE_AUDIT_READ,
        "jobs:write",
        "inventory:write",
        "schedules:write",
        "backups:write",
        "network:write",
        "config:write",
        SCOPE_INTEGRATIONS_WRITE,
        SCOPE_TEMPLATES_WRITE,
        SCOPE_HISTORY_WRITE,
    ] {
        assert!(
            operator_scopes.iter().any(|scope| scope == expected),
            "operator default scopes missing {expected}"
        );
    }

    assert_eq!(
        default_operator_scopes("viewer"),
        vec![SCOPE_FLEET_READ.to_string()]
    );
    assert_eq!(default_operator_scopes("admin"), vec!["*".to_string()]);
}

#[tokio::test]
async fn fleet_read_only_cannot_read_sensitive_payload_surfaces() {
    let state = memory_test_state();
    let (no_fleet_token, _) =
        issue_test_operator_headers(&state, "viewer", &[SCOPE_JOBS_READ]).await;
    let (_, viewer_headers) =
        issue_test_operator_headers(&state, "viewer", &[SCOPE_FLEET_READ]).await;
    let job_id = Uuid::new_v4();
    let terminal_id = Uuid::new_v4();

    assert!(!routes_ws::authenticate_socket_token(&state, &no_fleet_token).await);
    assert_scope_forbidden(
        routes_job_history::list_job_outputs(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path(job_id),
            axum::extract::Query(Default::default()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::download_file_download_bundle(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path(job_id),
            axum::extract::Query(routes_job_history::FileDownloadBundleQuery { clients: None }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::download_job_output_archive(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path(job_id),
            axum::extract::Query(routes_job_history::FileDownloadBundleQuery { clients: None }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::download_file_download_for_client(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path((job_id, "client-a".to_string())),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::download_job_output_stream(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path((job_id, "client-a".to_string())),
            axum::extract::Query(routes_job_history::JobOutputDownloadQuery {
                stream: "stdout".to_string(),
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::download_job_output_chunk(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path((job_id, "client-a".to_string(), 0)),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::compare_job_outputs(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path(job_id),
            axum::extract::Query(JobOutputComparisonQuery { mode: None }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::list_process_supervisor_inventory(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(HistoryQuery { limit: None }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::list_audit_logs(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::get_audit_log(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path(Uuid::new_v4()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_job_history::list_network_observations(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(NetworkEvidenceQuery::default()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_network::list_network_ospf_update_plans(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(HistoryQuery { limit: None }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_migrations::list_migration_links(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_alerts::list_fleet_alerts(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(FleetAlertQuery {
                limit: None,
                client_id: None,
                severity: None,
                category: None,
                operator_state: None,
                include_muted: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_history::export_history(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(crate::model_history::HistoryExportQuery {
                domains: None,
                limit: None,
                client_id: None,
                job_id: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_history::export_history(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(crate::model_history::HistoryExportQuery {
                domains: Some("audit_logs".to_string()),
                limit: None,
                client_id: None,
                job_id: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_history::export_history(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(crate::model_history::HistoryExportQuery {
                domains: Some("network_observations".to_string()),
                limit: None,
                client_id: None,
                job_id: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_history::export_history(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(crate::model_history::HistoryExportQuery {
                domains: Some("job_outputs".to_string()),
                limit: None,
                client_id: None,
                job_id: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_history::export_history(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(crate::model_history::HistoryExportQuery {
                domains: Some("backup_artifacts".to_string()),
                limit: None,
                client_id: None,
                job_id: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_backups::list_backup_requests(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_backups::list_backup_artifacts(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_backups::list_backup_policies(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_backups::download_backup_artifact(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path(Uuid::new_v4()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_restores::list_restore_plans(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_terminal_sessions::list_terminal_sessions(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(routes_terminal_sessions::TerminalSessionQuery {
                limit: None,
                client_id: None,
                session_id: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_terminal_sessions::terminal_session_replay(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path(("client-a".to_string(), terminal_id)),
            axum::extract::Query(routes_terminal_sessions::TerminalReplayQuery {
                from_seq: None,
                limit: None,
                max_bytes: None,
                include_data: Some(false),
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_webhook_rules::list_webhook_rules(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(crate::model_webhook_rules::WebhookRuleQuery {
                limit: None,
                enabled: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_webhook_rules::dry_run_webhook_rule(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::Json(crate::model_webhook_rules::WebhookRuleDryRunRequest {
                name: None,
                enabled: Some(true),
                expression: "status = online".to_string(),
                target: Some("https://www.cloudflare.com/vpsman-test-webhook".to_string()),
                event_kind: "manual.dry_run".to_string(),
                event_id: None,
                body_template: String::new(),
                cooldown_secs: None,
                notes: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_webhook_rules::list_webhook_rule_deliveries(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(crate::model_webhook_rules::WebhookRuleDeliveryQuery {
                limit: None,
                rule_id: None,
                event_kind: None,
                status: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_alerts::list_fleet_alert_notification_channels(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(FleetAlertNotificationChannelQuery {
                limit: None,
                enabled: None,
                scope_kind: None,
                scope_value: None,
                delivery_kind: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_alerts::list_fleet_alert_notifications(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(FleetAlertNotificationDeliveryQuery {
                limit: None,
                channel_id: None,
                alert_id: None,
                status: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_command_templates::list_command_templates(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(CommandTemplateQuery {
                limit: None,
                scope_kind: None,
                scope_value: None,
                command_type: None,
                display_group: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_schedules::list_schedules(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_configuration_presets::list_configuration_presets(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(ConfigurationPresetQuery { behavior: None }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_inventory::list_runtime_config_patch_generators(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_update_releases::list_agent_update_releases(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(HistoryQuery { limit: None }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_update_releases::latest_agent_update_release(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(routes_update_releases::LatestReleaseQuery {
                name: "vpsman-agent".to_string(),
                channel: "stable".to_string(),
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_file_transfers::list_file_transfer_sessions(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(routes_file_transfers::FileTransferSessionQuery {
                limit: None,
                client_id: None,
                session_id: None,
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_file_transfers::list_file_transfer_source_artifacts(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(HistoryQuery { limit: None }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_file_transfers::download_file_transfer_source_artifact(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path(Uuid::new_v4()),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_file_transfers::download_file_transfer_handoff(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Path(("client-a".to_string(), Uuid::new_v4())),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_network::list_tunnel_plans(axum::extract::State(state.clone()), viewer_headers)
            .await,
    );
}

#[tokio::test]
async fn fleet_websocket_heartbeat_revalidates_and_rejects_revoked_sessions() {
    let state = memory_test_state();
    let (fleet_token, _) = issue_test_operator_headers(&state, "viewer", &[SCOPE_FLEET_READ]).await;
    let context = routes_ws::authenticate_socket_context(&state, &fleet_token)
        .await
        .expect("initial websocket auth");
    let heartbeat = routes_ws::authenticated_heartbeat_message(&state, &fleet_token)
        .await
        .expect("authenticated websocket heartbeat");
    assert!(matches!(
        heartbeat,
        axum::extract::ws::Message::Ping(payload) if payload.is_empty()
    ));

    state
        .repo
        .revoke_operator_session(
            context
                .audit_session_id()
                .expect("authenticated websocket context has a session"),
            &context,
        )
        .await
        .unwrap();

    assert!(
        routes_ws::authenticated_heartbeat_message(&state, &fleet_token)
            .await
            .is_none()
    );
    assert!(routes_ws::authenticate_socket_context(&state, &fleet_token)
        .await
        .is_none());
}

#[tokio::test]
async fn revoke_operator_session_returns_the_exact_session_beyond_the_list_cap() {
    let state = memory_test_state();
    let (actor_token, _) = issue_test_operator_headers(&state, "admin", &["*"]).await;
    let actor = state
        .repo
        .authenticate_access_token(&actor_token)
        .await
        .unwrap()
        .unwrap();
    let target_auth = state
        .repo
        .issue_session(actor.operator.clone())
        .await
        .unwrap();
    let target = state
        .repo
        .authenticate_access_token(&target_auth.access_token)
        .await
        .unwrap()
        .unwrap();
    for _ in 0..200 {
        state
            .repo
            .issue_session(actor.operator.clone())
            .await
            .unwrap();
    }
    let Repository::Memory(memory) = &state.repo else {
        unreachable!("test uses memory repository");
    };
    for (index, session) in memory.sessions.write().await.iter_mut().enumerate() {
        session.created_unix = index as u64;
    }
    let actor_session_id = actor
        .audit_session_id()
        .expect("authenticated actor has a session");
    let target_session_id = target
        .audit_session_id()
        .expect("authenticated target has a session");
    assert!(!state
        .repo
        .list_operator_sessions(200, actor_session_id)
        .await
        .unwrap()
        .iter()
        .any(|session| session.id == target_session_id));

    let revoked = state
        .repo
        .revoke_operator_session(target_session_id, &actor)
        .await
        .unwrap()
        .expect("exact session lookup should return the revoked session");
    assert_eq!(revoked.id, target_session_id);
    assert!(revoked.revoked);
}

#[tokio::test]
async fn matching_sensitive_read_scopes_cross_authorization_boundary() {
    let state = memory_test_state();
    let (fleet_token, _) = issue_test_operator_headers(&state, "viewer", &[SCOPE_FLEET_READ]).await;
    let (_, jobs_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_JOBS_READ]).await;
    let (_, backups_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_BACKUPS_READ]).await;
    let (_, terminal_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_TERMINAL_READ]).await;
    let (_, integrations_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_INTEGRATIONS_READ]).await;
    let (_, templates_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_TEMPLATES_READ]).await;
    let (_, schedules_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_SCHEDULES_READ]).await;
    let (_, config_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_CONFIG_READ]).await;
    let (_, network_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_NETWORK_READ]).await;
    let (_, audit_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_AUDIT_READ]).await;

    assert!(routes_ws::authenticate_socket_token(&state, &fleet_token).await);
    assert_not_scope_forbidden(
        routes_job_history::list_job_outputs(
            axum::extract::State(state.clone()),
            jobs_headers.clone(),
            axum::extract::Path(Uuid::new_v4()),
            axum::extract::Query(Default::default()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_history::export_history(
            axum::extract::State(state.clone()),
            jobs_headers.clone(),
            axum::extract::Query(crate::model_history::HistoryExportQuery {
                domains: Some("job_outputs".to_string()),
                limit: None,
                client_id: None,
                job_id: None,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_history::export_history(
            axum::extract::State(state.clone()),
            backups_headers.clone(),
            axum::extract::Query(crate::model_history::HistoryExportQuery {
                domains: Some("backup_artifacts".to_string()),
                limit: None,
                client_id: None,
                job_id: None,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_history::export_history(
            axum::extract::State(state.clone()),
            audit_headers.clone(),
            axum::extract::Query(crate::model_history::HistoryExportQuery {
                domains: Some("audit_logs".to_string()),
                limit: None,
                client_id: None,
                job_id: None,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_job_history::list_audit_logs(
            axum::extract::State(state.clone()),
            audit_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_job_history::get_audit_log(
            axum::extract::State(state.clone()),
            audit_headers,
            axum::extract::Path(Uuid::new_v4()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_terminal_sessions::list_terminal_sessions(
            axum::extract::State(state.clone()),
            terminal_headers,
            axum::extract::Query(routes_terminal_sessions::TerminalSessionQuery {
                limit: None,
                client_id: None,
                session_id: None,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_webhook_rules::list_webhook_rules(
            axum::extract::State(state.clone()),
            integrations_headers.clone(),
            axum::extract::Query(crate::model_webhook_rules::WebhookRuleQuery {
                limit: None,
                enabled: None,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_alerts::list_fleet_alert_notification_channels(
            axum::extract::State(state.clone()),
            integrations_headers,
            axum::extract::Query(FleetAlertNotificationChannelQuery {
                limit: None,
                enabled: None,
                scope_kind: None,
                scope_value: None,
                delivery_kind: None,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_command_templates::list_command_templates(
            axum::extract::State(state.clone()),
            templates_headers,
            axum::extract::Query(CommandTemplateQuery {
                limit: None,
                scope_kind: None,
                scope_value: None,
                command_type: None,
                display_group: None,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_schedules::list_schedules(
            axum::extract::State(state.clone()),
            schedules_headers,
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_configuration_presets::list_configuration_presets(
            axum::extract::State(state.clone()),
            config_headers.clone(),
            axum::extract::Query(ConfigurationPresetQuery { behavior: None }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_update_releases::list_agent_update_releases(
            axum::extract::State(state.clone()),
            config_headers.clone(),
            axum::extract::Query(HistoryQuery { limit: None }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_update_releases::latest_agent_update_release(
            axum::extract::State(state.clone()),
            config_headers.clone(),
            axum::extract::Query(routes_update_releases::LatestReleaseQuery {
                name: "vpsman-agent".to_string(),
                channel: "stable".to_string(),
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_file_transfers::list_file_transfer_sessions(
            axum::extract::State(state.clone()),
            jobs_headers.clone(),
            axum::extract::Query(routes_file_transfers::FileTransferSessionQuery {
                limit: None,
                client_id: None,
                session_id: None,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_file_transfers::list_file_transfer_source_artifacts(
            axum::extract::State(state.clone()),
            jobs_headers.clone(),
            axum::extract::Query(HistoryQuery { limit: None }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_file_transfers::download_file_transfer_source_artifact(
            axum::extract::State(state.clone()),
            jobs_headers.clone(),
            axum::extract::Path(Uuid::new_v4()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_file_transfers::download_file_transfer_handoff(
            axum::extract::State(state.clone()),
            jobs_headers,
            axum::extract::Path(("client-a".to_string(), Uuid::new_v4())),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_backups::list_backup_requests(
            axum::extract::State(state.clone()),
            backups_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_backups::list_backup_artifacts(
            axum::extract::State(state.clone()),
            backups_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_backups::list_backup_policies(
            axum::extract::State(state.clone()),
            backups_headers.clone(),
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_backups::download_backup_artifact(
            axum::extract::State(state.clone()),
            backups_headers.clone(),
            axum::extract::Path(Uuid::new_v4()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_restores::list_restore_plans(
            axum::extract::State(state.clone()),
            backups_headers,
            axum::extract::Query(ListQuery::default()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_job_history::list_network_observations(
            axum::extract::State(state.clone()),
            network_headers.clone(),
            axum::extract::Query(NetworkEvidenceQuery::default()),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_network::list_network_ospf_update_plans(
            axum::extract::State(state.clone()),
            network_headers.clone(),
            axum::extract::Query(HistoryQuery { limit: None }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_history::export_history(
            axum::extract::State(state.clone()),
            network_headers.clone(),
            axum::extract::Query(crate::model_history::HistoryExportQuery {
                domains: Some("topology_history".to_string()),
                limit: None,
                client_id: None,
                job_id: None,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_network::list_tunnel_plans(axum::extract::State(state), network_headers).await,
    );
}

#[tokio::test]
async fn domain_write_surfaces_require_domain_authority() {
    let state = memory_test_state();
    let (_, history_only_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_HISTORY_WRITE]).await;
    let (_, history_jobs_headers) =
        issue_test_operator_headers(&state, "operator", &[SCOPE_HISTORY_WRITE, "jobs:write"]).await;
    let (_, jobs_write_headers) =
        issue_test_operator_headers(&state, "operator", &["jobs:write"]).await;
    let (_, backups_write_headers) =
        issue_test_operator_headers(&state, "operator", &["backups:write"]).await;

    assert_scope_forbidden(
        routes_history::upsert_history_retention_policy(
            axum::extract::State(state.clone()),
            history_only_headers,
            axum::Json(crate::model_history::UpsertHistoryRetentionPolicyRequest {
                domain: "job_outputs".to_string(),
                retention_days: Some(30),
                prune_limit: Some(100),
                enabled: Some(true),
                metadata_only: Some(true),
                export_enabled: Some(true),
                notes: None,
                clear_notes: false,
                confirmed: true,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_history::upsert_history_retention_policy(
            axum::extract::State(state.clone()),
            history_jobs_headers,
            axum::Json(crate::model_history::UpsertHistoryRetentionPolicyRequest {
                domain: "job_outputs".to_string(),
                retention_days: Some(30),
                prune_limit: Some(100),
                enabled: Some(true),
                metadata_only: Some(true),
                export_enabled: Some(true),
                notes: None,
                clear_notes: false,
                confirmed: true,
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_server_jobs::preview_artifact_cleanup(
            axum::extract::State(state.clone()),
            jobs_write_headers.clone(),
            axum::Json(crate::model::ArtifactCleanupPreviewRequest {
                expression: "artifact.domain = \"file_transfer_source\"".to_string(),
                domains: vec!["file_transfer".to_string()],
            }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_server_jobs::preview_artifact_cleanup(
            axum::extract::State(state.clone()),
            jobs_write_headers,
            axum::Json(crate::model::ArtifactCleanupPreviewRequest {
                expression: "artifact.domain = \"backup_artifact\"".to_string(),
                domains: vec!["backup_artifact".to_string()],
            }),
        )
        .await,
    );
    assert_not_scope_forbidden(
        routes_server_jobs::preview_artifact_cleanup(
            axum::extract::State(state),
            backups_write_headers,
            axum::Json(crate::model::ArtifactCleanupPreviewRequest {
                expression: "artifact.domain = \"backup_artifact\"".to_string(),
                domains: vec!["backup_artifact".to_string()],
            }),
        )
        .await,
    );
}

#[tokio::test]
async fn admin_can_create_sanitized_operator_record() {
    let repo = Repository::Memory(MemoryState::default());
    let admin = AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "admin".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Some(Uuid::new_v4()),
    };
    repo.create_operator(
        &CreateOperatorRequest {
            username: "viewer-a".to_string(),
            password: "viewer-password-123".to_string(),
            role: "viewer".to_string(),
            scopes: Vec::new(),
            session_refresh_ttl_secs: None,
            confirmed: true,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        },
        &admin,
    )
    .await
    .unwrap();

    let operators = repo.list_operators().await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();
    assert_eq!(operators.len(), 1);
    assert_eq!(operators[0].username, "viewer-a");
    assert_eq!(operators[0].role, "viewer");
    assert_eq!(audits[0].action, "operator.created");
    assert!(!serde_json::to_string(&audits[0].metadata)
        .unwrap()
        .contains("viewer-password-123"));
}

#[tokio::test]
async fn admin_user_routes_require_admin_risk_acknowledgement() {
    let state = memory_privilege_test_state();
    let (_admin, headers) = crate::test_auth_context_and_headers(&state).await;

    let error = routes_auth::create_operator(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(CreateOperatorRequest {
            username: "second-admin".to_string(),
            password: "second-admin-password-123".to_string(),
            role: "admin".to_string(),
            scopes: Vec::new(),
            session_refresh_ttl_secs: None,
            confirmed: true,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "admin_risk_acknowledgement_required");

    let created = routes_auth::create_operator(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(CreateOperatorRequest {
            username: "second-admin".to_string(),
            password: "second-admin-password-123".to_string(),
            role: "admin".to_string(),
            scopes: Vec::new(),
            session_refresh_ttl_secs: Some(crate::DEFAULT_REFRESH_TOKEN_TTL_SECS),
            confirmed: true,
            admin_risk_acknowledged: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(created.role, "admin");

    let error = routes_auth::disable_operator(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(created.id),
        axum::Json(OperatorLifecycleRequest {
            confirmed: true,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "admin_risk_acknowledgement_required");

    let disabled = routes_auth::disable_operator(
        axum::extract::State(state),
        headers,
        axum::extract::Path(created.id),
        axum::Json(OperatorLifecycleRequest {
            confirmed: true,
            admin_risk_acknowledged: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(disabled.status, "disabled");
    assert!(disabled.disabled_at.is_some());
}

#[tokio::test]
async fn admin_user_routes_preserve_one_active_admin() {
    let state = memory_privilege_test_state();
    let (admin, headers) = crate::test_auth_context_and_headers(&state).await;

    let error = routes_auth::update_operator(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(admin.operator.id),
        axum::Json(UpdateOperatorRequest {
            role: "operator".to_string(),
            scopes: Vec::new(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            confirmed: true,
            admin_risk_acknowledged: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "last_active_admin_required");

    let error = routes_auth::disable_operator(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(admin.operator.id),
        axum::Json(OperatorLifecycleRequest {
            confirmed: true,
            admin_risk_acknowledged: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "last_active_admin_required");

    let error = routes_auth::delete_operator(
        axum::extract::State(state),
        headers,
        axum::extract::Path(admin.operator.id),
        axum::Json(OperatorLifecycleRequest {
            confirmed: true,
            admin_risk_acknowledged: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "last_active_admin_required");
}

#[tokio::test]
async fn operator_management_routes_require_confirmation_and_privilege() {
    let state = memory_gateway_test_state();
    let (admin, headers) = crate::test_auth_context_and_headers(&state).await;

    let error = routes_auth::create_operator(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(CreateOperatorRequest {
            username: "unconfirmed-operator".to_string(),
            password: "operator-password-123".to_string(),
            role: "operator".to_string(),
            scopes: Vec::new(),
            session_refresh_ttl_secs: None,
            confirmed: false,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "confirmation_required");

    let error = routes_auth::create_operator(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(CreateOperatorRequest {
            username: "missing-privilege".to_string(),
            password: "operator-password-123".to_string(),
            role: "operator".to_string(),
            scopes: Vec::new(),
            session_refresh_ttl_secs: None,
            confirmed: true,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(error.code, "privilege_assertion_required");

    let target = state
        .repo
        .create_operator(
            &CreateOperatorRequest {
                username: "route-target".to_string(),
                password: "operator-password-123".to_string(),
                role: "operator".to_string(),
                scopes: Vec::new(),
                session_refresh_ttl_secs: None,
                confirmed: true,
                admin_risk_acknowledged: false,
                privilege_assertion: None,
            },
            &admin,
        )
        .await
        .unwrap();

    let error = routes_auth::update_operator(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(target.id),
        axum::Json(UpdateOperatorRequest {
            role: "viewer".to_string(),
            scopes: Vec::new(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            confirmed: true,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(error.code, "privilege_assertion_required");

    let error = routes_auth::disable_operator(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(target.id),
        axum::Json(OperatorLifecycleRequest {
            confirmed: true,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(error.code, "privilege_assertion_required");

    let error = routes_auth::reset_operator_password(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(target.id),
        axum::Json(OperatorPasswordResetRequest {
            password: "replacement-password-123".to_string(),
            confirmed: true,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(error.code, "privilege_assertion_required");

    let error = routes_auth::clear_operator_totp(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(target.id),
        axum::Json(OperatorLifecycleRequest {
            confirmed: true,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(error.code, "privilege_assertion_required");

    let issued = state.repo.issue_session(target).await.unwrap();
    let session = state
        .repo
        .authenticate_access_token(&issued.access_token)
        .await
        .unwrap()
        .unwrap();
    let error = routes_auth::revoke_operator_session(
        axum::extract::State(state),
        headers,
        axum::extract::Path(
            session
                .audit_session_id()
                .expect("authenticated session has an ID"),
        ),
        axum::Json(OperatorSessionRevokeRequest {
            confirmed: true,
            admin_risk_acknowledged: false,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert_eq!(error.code, "privilege_assertion_required");
}

#[tokio::test]
async fn job_cancel_routes_require_explicit_confirmation() {
    let state = memory_test_state();
    let (_admin, headers) = crate::test_auth_context_and_headers(&state).await;

    let error = routes_jobs::cancel_job(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(Uuid::new_v4()),
        axum::Json(CancelJobRequest {
            reason: Some("operator review".to_string()),
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "job_cancel_requires_confirmation");

    let error = routes_server_jobs::cancel_server_job(
        axum::extract::State(state),
        headers,
        axum::extract::Path(Uuid::new_v4()),
        axum::Json(routes_server_jobs::CancelServerJobRequest { confirmed: false }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "server_job_cancel_requires_confirmation");
}

#[tokio::test]
async fn disabled_and_deleted_operators_cannot_login_and_deleted_usernames_remain_reserved() {
    let repo = Repository::Memory(MemoryState::default());
    let admin_auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();
    let admin = AuthContext {
        operator: admin_auth.operator.clone(),
        session_id: Some(Uuid::new_v4()),
    };
    let created = repo
        .create_operator(
            &CreateOperatorRequest {
                username: "ops-a".to_string(),
                password: "ops-password-123".to_string(),
                role: "operator".to_string(),
                scopes: Vec::new(),
                session_refresh_ttl_secs: Some(86_400),
                confirmed: true,
                admin_risk_acknowledged: false,
                privilege_assertion: None,
            },
            &admin,
        )
        .await
        .unwrap();
    let login = repo
        .login_operator(&LoginRequest {
            username: "ops-a".to_string(),
            password: "ops-password-123".to_string(),
            totp_code: None,
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(login.refresh_expires_in_secs, 86_400);

    let disabled = repo
        .set_operator_status(created.id, "disabled", &admin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disabled.status, "disabled");
    assert!(repo
        .authenticate_access_token(&login.access_token)
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .login_operator(&LoginRequest {
            username: "ops-a".to_string(),
            password: "ops-password-123".to_string(),
            totp_code: None,
        })
        .await
        .unwrap()
        .is_none());

    let deleted = repo
        .set_operator_status(created.id, "deleted", &admin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deleted.status, "deleted");
    assert!(deleted.deleted_at.is_some());
    assert!(repo
        .create_operator(
            &CreateOperatorRequest {
                username: "ops-a".to_string(),
                password: "new-ops-password-123".to_string(),
                role: "operator".to_string(),
                scopes: Vec::new(),
                session_refresh_ttl_secs: None,
                confirmed: true,
                admin_risk_acknowledged: false,
                privilege_assertion: None,
            },
            &admin,
        )
        .await
        .is_err());
}

#[tokio::test]
async fn operator_preferences_update_persists_to_authenticated_views() {
    let repo = Repository::Memory(MemoryState::default());
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator,
        session_id: Some(Uuid::new_v4()),
    };

    let preferences = OperatorPreferences {
        language: "en".to_string(),
        review_prompt_mode: "overlay".to_string(),
        sidebar_subpanel_default: "all".to_string(),
        timezone: Some("UTC".to_string()),
        vps_name_display_mode: "name".to_string(),
        fleet_tag_visibility_overrides: BTreeMap::from([("provider:alpha".to_string(), true)]),
        gateway_server_public_key_hex: Some("11".repeat(32)),
        gateway_endpoints: "primary=gw.example.com:9443=10".to_string(),
        ..OperatorPreferences::default()
    };
    let updated = repo
        .update_operator_preferences(&actor, preferences)
        .await
        .unwrap();
    assert_eq!(updated.preferences.vps_name_display_mode, "name");
    assert_eq!(updated.preferences.timezone.as_deref(), Some("UTC"));
    assert_eq!(updated.preferences.sidebar_subpanel_default, "all");
    assert_eq!(updated.preferences.review_prompt_mode, "overlay");
    assert_eq!(updated.preferences.bulk_output_compare_mode, "binary");
    assert_eq!(
        updated.preferences.gateway_server_public_key_hex.as_deref(),
        Some("11".repeat(32).as_str())
    );
    assert_eq!(
        updated.preferences.gateway_endpoints,
        "primary=gw.example.com:9443=10"
    );
    assert_eq!(
        updated
            .preferences
            .fleet_tag_visibility_overrides
            .get("provider:alpha"),
        Some(&true)
    );

    let context = repo
        .authenticate_access_token(&auth.access_token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(context.operator.preferences.vps_name_display_mode, "name");
    assert_eq!(
        context.operator.preferences.timezone.as_deref(),
        Some("UTC")
    );
    assert_eq!(context.operator.preferences.sidebar_subpanel_default, "all");
    assert_eq!(context.operator.preferences.review_prompt_mode, "overlay");
    assert_eq!(
        context.operator.preferences.bulk_output_compare_mode,
        "binary"
    );
    assert_eq!(
        context
            .operator
            .preferences
            .fleet_tag_visibility_overrides
            .get("provider:alpha"),
        Some(&true)
    );
}

#[tokio::test]
async fn operator_preferences_route_rejects_invalid_values() {
    let state = memory_test_state();
    let cases = [
        (
            OperatorPreferences {
                vps_name_display_mode: "id_only".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_vps_name_display_mode",
        ),
        (
            OperatorPreferences {
                language: "fr".to_string(),
                ..OperatorPreferences::default()
            },
            "unsupported_operator_language",
        ),
        (
            OperatorPreferences {
                sidebar_subpanel_default: "everything".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_sidebar_subpanel_default",
        ),
        (
            OperatorPreferences {
                review_prompt_mode: "floating".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_review_prompt_mode",
        ),
        (
            OperatorPreferences {
                timezone: Some("Mars/Base".to_string()),
                ..OperatorPreferences::default()
            },
            "invalid_timezone",
        ),
        (
            OperatorPreferences {
                bulk_output_compare_mode: "loose".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_bulk_output_compare_mode",
        ),
        (
            OperatorPreferences {
                gateway_server_public_key_hex: Some("gg".repeat(32)),
                ..OperatorPreferences::default()
            },
            "invalid_gateway_server_public_key_hex",
        ),
        (
            OperatorPreferences {
                gateway_server_public_key_hex: Some("aa".repeat(31)),
                ..OperatorPreferences::default()
            },
            "invalid_gateway_server_public_key_hex",
        ),
        (
            OperatorPreferences {
                gateway_endpoints: "bad-format".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_gateway_endpoints",
        ),
        (
            OperatorPreferences {
                gateway_endpoints: "primary=999.0.0.1:9443=10".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_gateway_endpoints",
        ),
        (
            OperatorPreferences {
                gateway_endpoints: "primary=001.2.3.4:9443=10".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_gateway_endpoints",
        ),
        (
            OperatorPreferences {
                gateway_endpoints: "primary=[::ffff:001.2.3.4]:9443=10".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_gateway_endpoints",
        ),
        (
            OperatorPreferences {
                gateway_endpoints: "primary=gw.example.com:+9443=10".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_gateway_endpoints",
        ),
        (
            OperatorPreferences {
                gateway_endpoints: "primary=gw.example.com:9443=+10".to_string(),
                ..OperatorPreferences::default()
            },
            "invalid_gateway_endpoints",
        ),
        (
            OperatorPreferences {
                fleet_tag_visibility_overrides: BTreeMap::from([("bad tag".to_string(), true)]),
                ..OperatorPreferences::default()
            },
            "invalid_fleet_tag_visibility_tag",
        ),
    ];

    for (preferences, expected_code) in cases {
        let error = routes_auth::update_operator_preferences(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            axum::Json(preferences),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, expected_code);
    }
}

#[tokio::test]
async fn operator_preferences_route_persists_valid_payload() {
    let state = memory_test_state();
    let headers = crate::test_auth_headers(&state).await;

    let response = routes_auth::update_operator_preferences(
        axum::extract::State(state),
        headers,
        axum::Json(OperatorPreferences {
            gateway_endpoints: "primary=gw.example.com:9443=10,backup=[2001:db8::5]:9443=20,edge=[::ffff:192.0.2.4]:443=0".to_string(),
            gateway_server_public_key_hex: Some("AA".repeat(32)),
            language: "en".to_string(),
            review_prompt_mode: "overlay".to_string(),
            sidebar_subpanel_default: "all".to_string(),
            timezone: Some(" America/Los_Angeles ".to_string()),
            vps_name_display_mode: "name".to_string(),
            ..OperatorPreferences::default()
        }),
    )
    .await
    .unwrap();

    assert_eq!(response.0.preferences.vps_name_display_mode, "name");
    assert_eq!(
        response.0.preferences.timezone.as_deref(),
        Some("America/Los_Angeles")
    );
    assert_eq!(response.0.preferences.sidebar_subpanel_default, "all");
    assert_eq!(response.0.preferences.review_prompt_mode, "overlay");
    assert_eq!(
        response.0.preferences.gateway_endpoints,
        "primary=gw.example.com:9443=10\nbackup=[2001:db8::5]:9443=20\nedge=[::ffff:192.0.2.4]:443=0"
    );
    assert_eq!(
        response
            .0
            .preferences
            .gateway_server_public_key_hex
            .as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
}

#[tokio::test]
async fn memory_repository_routes_require_bearer_tokens() {
    let state = memory_test_state();
    let missing_headers = HeaderMap::new();

    assert_missing_bearer(state.require_operator(&missing_headers).await.unwrap_err());
    assert_missing_bearer(
        state
            .require_operator_scope(&missing_headers, "fleet:read")
            .await
            .unwrap_err(),
    );
    assert_missing_bearer(
        state
            .require_operator_role_and_scope(&missing_headers, "operator", "jobs:write")
            .await
            .unwrap_err(),
    );
    assert_missing_bearer(
        routes_auth::current_operator(axum::extract::State(state.clone()), HeaderMap::new())
            .await
            .unwrap_err(),
    );
    assert_missing_bearer(
        routes_inventory::list_agents(axum::extract::State(state.clone()), HeaderMap::new())
            .await
            .unwrap_err(),
    );
    assert_missing_bearer(
        routes_alerts::list_fleet_alerts(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            axum::extract::Query(FleetAlertQuery {
                limit: None,
                client_id: None,
                severity: None,
                category: None,
                operator_state: None,
                include_muted: None,
            }),
        )
        .await
        .unwrap_err(),
    );
    assert_missing_bearer(
        routes_jobs::create_job(
            axum::extract::State(state.clone()),
            HeaderMap::new(),
            axum::Json(CreateJobRequest {
                job_id: None,
                selector_expression: "id:client-a".to_string(),
                target_client_ids: vec!["client-a".to_string()],
                destructive: false,
                confirmed: true,
                command: "uptime".to_string(),
                argv: Vec::new(),
                operation: None,
                max_timeout_secs: None,
                force_unprivileged: false,
                privileged: false,
                privilege_assertion: None,
                rollout: None,
            }),
        )
        .await
        .unwrap_err(),
    );
    assert_missing_bearer(
        routes_webhook_rules::upsert_webhook_rule(
            axum::extract::State(state),
            HeaderMap::new(),
            axum::Json(crate::model_webhook_rules::CreateWebhookRuleRequest {
                id: None,
                name: "route auth regression".to_string(),
                enabled: true,
                expression: "status = online".to_string(),
                target: "https://www.cloudflare.com/vpsman-test-webhook".to_string(),
                body_template: String::new(),
                signing_secret: None,
                clear_signing_secret: false,
                cooldown_secs: Some(60),
                notes: None,
                confirmed: true,
            }),
        )
        .await
        .unwrap_err(),
    );
}

fn assert_missing_bearer(error: ApiError) {
    assert_eq!(error.status, StatusCode::UNAUTHORIZED);
    assert_eq!(error.code, "missing_bearer_token");
}

#[test]
fn stored_operator_preferences_drop_invalid_timezone() {
    let preferences = repository_auth::parse_operator_preferences(serde_json::json!({
        "language": "en",
        "sidebar_subpanel_default": "all",
        "timezone": "Mars/Base",
        "vps_name_display_mode": "name"
    }));

    assert_eq!(preferences.vps_name_display_mode, "name");
    assert_eq!(preferences.sidebar_subpanel_default, "all");
    assert_eq!(preferences.timezone, None);
}

#[tokio::test]
async fn repeated_totp_setup_reuses_pending_secret_without_enabling() {
    let repo = Repository::Memory(MemoryState::default());
    let password = "admin-password-123";
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator,
        session_id: Some(Uuid::new_v4()),
    };
    let TotpSetupOutcome::Created(first) =
        repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected initial TOTP setup");
    };
    let stored_before = repo.operator_by_username("admin").await.unwrap().unwrap();
    let encrypted_before = stored_before
        .encrypted_totp_secret()
        .expect("pending encrypted TOTP secret");

    let TotpSetupOutcome::Created(second) =
        repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected pending TOTP setup");
    };
    let stored_after = repo.operator_by_username("admin").await.unwrap().unwrap();
    let encrypted_after = stored_after
        .encrypted_totp_secret()
        .expect("pending encrypted TOTP secret");

    assert_eq!(second.secret_base32, first.secret_base32);
    assert_eq!(second.otpauth_uri, first.otpauth_uri);
    assert_eq!(
        encrypted_after.ciphertext_hex,
        encrypted_before.ciphertext_hex
    );
    assert_eq!(encrypted_after.nonce_hex, encrypted_before.nonce_hex);
    assert_eq!(encrypted_after.salt_hex, encrypted_before.salt_hex);
    assert!(!stored_after.totp_enabled);
    assert_eq!(stored_after.totp_last_accepted_step, None);
}

#[tokio::test]
async fn operator_totp_lifecycle_encrypts_secret_and_gates_login() {
    let repo = Repository::Memory(MemoryState::default());
    let password = "admin-password-123";
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator.clone(),
        session_id: Some(Uuid::new_v4()),
    };
    let setup = repo.setup_operator_totp(&actor, password).await.unwrap();
    let TotpSetupOutcome::Created(setup) = setup else {
        panic!("expected TOTP setup");
    };
    assert_eq!(setup.algorithm, "SHA1");
    assert!(setup.otpauth_uri.starts_with("otpauth://totp/"));

    let encrypted = repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .encrypted_totp_secret()
        .expect("encrypted totp secret");
    assert!(!encrypted.ciphertext_hex.contains(&setup.secret_base32));
    let secret = crate::auth_totp::decrypt_totp_secret(password, &encrypted).unwrap();
    let current_step = unix_now() / crate::auth_totp::TOTP_PERIOD_SECS;
    let confirm_code = crate::auth_totp::totp_code_for_step(&secret, current_step);
    let login_code = crate::auth_totp::totp_code_for_step(&secret, current_step.saturating_add(1));

    assert!(matches!(
        repo.confirm_operator_totp(&actor, password, "000000")
            .await
            .unwrap(),
        TotpUpdateOutcome::InvalidCredentials
    ));
    let TotpUpdateOutcome::Updated(operator) = repo
        .confirm_operator_totp(&actor, password, &confirm_code)
        .await
        .unwrap()
    else {
        panic!("expected TOTP enabled");
    };
    assert!(operator.totp_enabled);
    assert!(matches!(
        repo.confirm_operator_totp(&actor, password, &login_code)
            .await
            .unwrap(),
        TotpUpdateOutcome::AlreadyEnabled
    ));

    assert!(repo
        .login_operator(&LoginRequest {
            username: "admin".to_string(),
            password: password.to_string(),
            totp_code: None,
        })
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .login_operator(&LoginRequest {
            username: "admin".to_string(),
            password: password.to_string(),
            totp_code: Some(confirm_code),
        })
        .await
        .unwrap()
        .is_none());
    let logged_in = repo
        .login_operator(&LoginRequest {
            username: "admin".to_string(),
            password: password.to_string(),
            totp_code: Some(login_code.clone()),
        })
        .await
        .unwrap()
        .expect("login with TOTP");
    assert!(logged_in.operator.totp_enabled);

    assert!(matches!(
        repo.disable_operator_totp(
            &AuthContext {
                operator: logged_in.operator.clone(),
                session_id: Some(Uuid::new_v4()),
            },
            password,
            &login_code,
        )
        .await
        .unwrap(),
        TotpUpdateOutcome::InvalidCredentials
    ));

    let audit_json = serde_json::to_string(&repo.list_audit_logs(10).await.unwrap()).unwrap();
    assert!(audit_json.contains("operator_totp.setup"));
    assert!(audit_json.contains("operator_totp.enabled"));
    assert!(!audit_json.contains(&setup.secret_base32));
}

#[tokio::test]
async fn operator_totp_disable_consumes_a_newer_code_and_clears_replay_state() {
    let repo = Repository::Memory(MemoryState::default());
    let password = "admin-password-123";
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator,
        session_id: Some(Uuid::new_v4()),
    };
    let TotpSetupOutcome::Created(_) = repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected TOTP setup");
    };
    let encrypted = repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .encrypted_totp_secret()
        .unwrap();
    let secret = crate::auth_totp::decrypt_totp_secret(password, &encrypted).unwrap();
    let current_step = unix_now() / crate::auth_totp::TOTP_PERIOD_SECS;
    let confirm_code = crate::auth_totp::totp_code_for_step(&secret, current_step);
    let disable_code =
        crate::auth_totp::totp_code_for_step(&secret, current_step.saturating_add(1));
    let TotpUpdateOutcome::Updated(enabled) = repo
        .confirm_operator_totp(&actor, password, &confirm_code)
        .await
        .unwrap()
    else {
        panic!("expected TOTP enabled");
    };
    let TotpUpdateOutcome::Updated(disabled) = repo
        .disable_operator_totp(
            &AuthContext {
                operator: *enabled,
                session_id: Some(Uuid::new_v4()),
            },
            password,
            &disable_code,
        )
        .await
        .unwrap()
    else {
        panic!("expected TOTP disabled");
    };
    assert!(!disabled.totp_enabled);
    let stored = repo.operator_by_username("admin").await.unwrap().unwrap();
    assert!(stored.encrypted_totp_secret().is_none());
    assert_eq!(stored.totp_last_accepted_step, None);
    assert!(
        serde_json::to_string(&repo.list_audit_logs(10).await.unwrap())
            .unwrap()
            .contains("operator_totp.disabled")
    );
}

#[tokio::test]
async fn concurrent_totp_login_consumes_one_code_once() {
    let repo = Repository::Memory(MemoryState::default());
    let password = "admin-password-123";
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator,
        session_id: Some(Uuid::new_v4()),
    };
    let TotpSetupOutcome::Created(_) = repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected TOTP setup");
    };
    let encrypted = repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .encrypted_totp_secret()
        .unwrap();
    let secret = crate::auth_totp::decrypt_totp_secret(password, &encrypted).unwrap();
    let current_step = unix_now() / crate::auth_totp::TOTP_PERIOD_SECS;
    let confirm_code = crate::auth_totp::totp_code_for_step(&secret, current_step);
    let login_step = current_step.saturating_add(1);
    let login_code = crate::auth_totp::totp_code_for_step(&secret, login_step);
    assert!(matches!(
        repo.confirm_operator_totp(&actor, password, &confirm_code)
            .await
            .unwrap(),
        TotpUpdateOutcome::Updated(_)
    ));

    let left_request = LoginRequest {
        username: "admin".to_string(),
        password: password.to_string(),
        totp_code: Some(login_code.clone()),
    };
    let right_request = LoginRequest {
        username: "admin".to_string(),
        password: password.to_string(),
        totp_code: Some(login_code),
    };
    let (left, right) = tokio::join!(
        repo.login_operator(&left_request),
        repo.login_operator(&right_request),
    );
    let accepted = [left.unwrap(), right.unwrap()]
        .into_iter()
        .filter(Option::is_some)
        .count();
    assert_eq!(accepted, 1);
    assert_eq!(
        repo.operator_by_username("admin")
            .await
            .unwrap()
            .unwrap()
            .totp_last_accepted_step,
        Some(login_step)
    );
}

#[tokio::test]
async fn operator_password_reset_clears_totp_secret_material() {
    let repo = Repository::Memory(MemoryState::default());
    let password = "admin-password-123";
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator.clone(),
        session_id: Some(Uuid::new_v4()),
    };

    let TotpSetupOutcome::Created(_) = repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected TOTP setup");
    };
    let operator = repo
        .operator_by_id(actor.operator.id)
        .await
        .unwrap()
        .unwrap();
    let encrypted = operator
        .encrypted_totp_secret()
        .expect("encrypted totp secret");
    let secret = crate::auth_totp::decrypt_totp_secret(password, &encrypted).unwrap();
    let code = crate::auth_totp::totp_code_for_step(&secret, unix_now() / 30);
    let TotpUpdateOutcome::Updated(enabled) = repo
        .confirm_operator_totp(&actor, password, &code)
        .await
        .unwrap()
    else {
        panic!("expected TOTP enabled");
    };
    assert!(enabled.totp_enabled);

    let reset = repo
        .reset_operator_password(actor.operator.id, "replacement-password-123", &actor)
        .await
        .unwrap()
        .unwrap();
    assert!(!reset.totp_enabled);
    let stored = repo
        .operator_by_id(actor.operator.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!stored.totp_enabled);
    assert!(stored.encrypted_totp_secret().is_none());
    assert_eq!(stored.totp_last_accepted_step, None);

    let login = repo
        .login_operator(&LoginRequest {
            username: "admin".to_string(),
            password: "replacement-password-123".to_string(),
            totp_code: None,
        })
        .await
        .unwrap()
        .expect("login after reset without stale TOTP");
    assert!(!login.operator.totp_enabled);
}

#[test]
fn internal_gateway_token_requires_matching_bearer() {
    let (events, _) = broadcast::channel(1);
    let state = AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: Some("gateway-secret-at-least-32-characters".to_string()),
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    };
    let missing = HeaderMap::new();
    let mut wrong = HeaderMap::new();
    wrong.insert(
        axum::http::header::AUTHORIZATION,
        "Bearer wrong-secret".parse().unwrap(),
    );
    let mut matching = HeaderMap::new();
    matching.insert(
        axum::http::header::AUTHORIZATION,
        "Bearer gateway-secret-at-least-32-characters"
            .parse()
            .unwrap(),
    );

    assert_eq!(
        state.require_internal_gateway(&missing).unwrap_err().status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        state.require_internal_gateway(&wrong).unwrap_err().status,
        StatusCode::UNAUTHORIZED
    );
    assert!(state.require_internal_gateway(&matching).is_ok());
}

#[test]
fn internal_token_startup_validation_rejects_missing_short_or_placeholder() {
    assert!(required_internal_token(None).is_err());
    assert!(required_internal_token(Some("short")).is_err());
    assert!(required_internal_token(Some("change-me-internal-token")).is_err());
    assert!(required_internal_token(Some("dev-internal-token-change-me-32chars")).is_err());
    assert!(required_internal_token(Some("replace-with-random-token-at-least-32-chars")).is_err());
    assert!(required_internal_token(Some("real-internal-token-value-32-plus-chars")).is_ok());
}

#[test]
fn api_startup_rejects_gateway_verifier_env() {
    assert_eq!(
        forbidden_api_privilege_env_var(|name| name == "VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX"),
        Some("VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX")
    );
}

#[test]
fn internal_gateway_token_is_mandatory_for_memory_repository() {
    let (events, _) = broadcast::channel(1);
    let state = AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    };

    assert_eq!(
        state
            .require_internal_gateway(&HeaderMap::new())
            .unwrap_err()
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert!(constant_time_eq(b"same", b"same"));
    assert!(!constant_time_eq(b"same", b"different"));
}

fn memory_test_state() -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: Some("gateway-secret-at-least-32-characters".to_string()),
        gateway: GatewayDispatchClient::default(),
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

fn memory_test_state_with_ip_throttle_limit(limit: i64) -> (AppState, std::path::PathBuf) {
    let mut state = memory_test_state();
    let suite_config_path =
        std::env::temp_dir().join(format!("vpsman-auth-throttle-test-{}.toml", Uuid::new_v4()));
    std::fs::write(
        &suite_config_path,
        format!("version = 1\n\n[api]\noperator_auth_ip_failed_attempt_limit = {limit}\n"),
    )
    .unwrap();
    state.suite_config_path = suite_config_path.clone();
    (state, suite_config_path)
}

fn memory_privilege_test_state() -> AppState {
    let mut state = memory_test_state();
    state.gateway = crate::gateway_client::GatewayDispatchClient::test_privilege_auto_approve();
    state
}

fn memory_gateway_test_state() -> AppState {
    let mut state = memory_test_state();
    state.gateway = crate::gateway_client::GatewayDispatchClient::new(
        Some("http://127.0.0.1:9".to_string()),
        Some("gateway-secret-at-least-32-characters".to_string()),
    );
    state
}

async fn issue_test_operator_headers(
    state: &AppState,
    role: &str,
    scopes: &[&str],
) -> (String, HeaderMap) {
    let operator = OperatorRecord {
        id: Uuid::new_v4(),
        username: format!("test-{role}-{}", Uuid::new_v4()),
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
    if let Repository::Memory(memory) = &state.repo {
        memory.operators.write().await.push(operator.clone());
    } else {
        panic!("issue_test_operator_headers supports only memory repository tests");
    }
    let auth = state
        .repo
        .issue_session(operator.view())
        .await
        .expect("test operator session");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        format!("Bearer {}", auth.access_token)
            .parse()
            .expect("test bearer header"),
    );
    (auth.access_token, headers)
}

fn assert_scope_forbidden<T>(result: Result<T, ApiError>) {
    match result {
        Err(error) => {
            assert_eq!(error.status, StatusCode::FORBIDDEN);
            assert_eq!(error.code, "operator_scope_insufficient");
        }
        Ok(_) => panic!("expected operator_scope_insufficient"),
    }
}

fn assert_not_scope_forbidden<T>(result: Result<T, ApiError>) {
    if let Err(error) = result {
        assert_ne!(error.code, "operator_scope_insufficient");
    }
}
