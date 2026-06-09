use super::*;
use axum::http::StatusCode;

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
        },
        session_id: Uuid::new_v4(),
    };
    repo.create_operator(
        &CreateOperatorRequest {
            username: "viewer-a".to_string(),
            password: "viewer-password-123".to_string(),
            role: "viewer".to_string(),
            scopes: Vec::new(),
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
        session_id: Uuid::new_v4(),
    };

    let preferences = OperatorPreferences {
        language: "en".to_string(),
        sidebar_subpanel_default: "all".to_string(),
        timezone: Some("UTC".to_string()),
        vps_name_display_mode: "name".to_string(),
        ..OperatorPreferences::default()
    };
    let updated = repo
        .update_operator_preferences(&actor, preferences)
        .await
        .unwrap();
    assert_eq!(updated.preferences.vps_name_display_mode, "name");
    assert_eq!(updated.preferences.timezone.as_deref(), Some("UTC"));
    assert_eq!(updated.preferences.sidebar_subpanel_default, "all");
    assert_eq!(updated.preferences.bulk_output_compare_mode, "binary");

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
    assert_eq!(
        context.operator.preferences.bulk_output_compare_mode,
        "binary"
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
    let operator = OperatorRecord {
        id: Uuid::nil(),
        username: "memory-dev".to_string(),
        password_hash: "not-used-in-debug-memory-mode".to_string(),
        role: "admin".to_string(),
        scopes: vec!["*".to_string()],
        preferences: OperatorPreferences::default(),
        totp_enabled: false,
        totp_secret_ciphertext_hex: None,
        totp_secret_nonce_hex: None,
        totp_secret_salt_hex: None,
    };
    if let Repository::Memory(memory) = &state.repo {
        memory.operators.write().await.push(operator);
    }

    let response = routes_auth::update_operator_preferences(
        axum::extract::State(state),
        HeaderMap::new(),
        axum::Json(OperatorPreferences {
            language: "en".to_string(),
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
        session_id: Uuid::new_v4(),
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
    let code = crate::auth_totp::totp_code_for_step(&secret, unix_now() / 30);

    assert!(matches!(
        repo.confirm_operator_totp(&actor, password, "000000")
            .await
            .unwrap(),
        TotpUpdateOutcome::InvalidCredentials
    ));
    let TotpUpdateOutcome::Updated(operator) = repo
        .confirm_operator_totp(&actor, password, &code)
        .await
        .unwrap()
    else {
        panic!("expected TOTP enabled");
    };
    assert!(operator.totp_enabled);

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
            totp_code: Some("111111".to_string()),
        })
        .await
        .unwrap()
        .is_none());
    let logged_in = repo
        .login_operator(&LoginRequest {
            username: "admin".to_string(),
            password: password.to_string(),
            totp_code: Some(code.clone()),
        })
        .await
        .unwrap()
        .expect("login with TOTP");
    assert!(logged_in.operator.totp_enabled);

    let TotpUpdateOutcome::Updated(disabled) = repo
        .disable_operator_totp(
            &AuthContext {
                operator: logged_in.operator,
                session_id: Uuid::new_v4(),
            },
            password,
            &code,
        )
        .await
        .unwrap()
    else {
        panic!("expected TOTP disabled");
    };
    assert!(!disabled.totp_enabled);
    assert!(repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .encrypted_totp_secret()
        .is_none());

    let audit_json = serde_json::to_string(&repo.list_audit_logs(10).await.unwrap()).unwrap();
    assert!(audit_json.contains("operator_totp.setup"));
    assert!(audit_json.contains("operator_totp.enabled"));
    assert!(audit_json.contains("operator_totp.disabled"));
    assert!(!audit_json.contains(&setup.secret_base32));
}

#[test]
fn internal_gateway_token_requires_matching_bearer() {
    let (events, _) = broadcast::channel(1);
    let state = AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: Some("gateway-secret-at-least-32-characters".to_string()),
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
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
    assert!(required_internal_token(Some("replace-with-random-token-at-least-32-chars")).is_err());
    assert!(required_internal_token(Some("real-internal-token-value-32-plus-chars")).is_ok());
}

#[test]
fn api_startup_rejects_gateway_verifier_env() {
    assert_eq!(
        forbidden_api_privilege_env_var(|name| name == "VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX"),
        Some("VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX")
    );
    assert_eq!(
        forbidden_api_privilege_env_var(|name| name == "VPSMAN_SERVER_SIGNING_KEY_HEX"),
        None
    );
}

#[test]
fn internal_gateway_token_is_mandatory_even_for_memory_dev() {
    let (events, _) = broadcast::channel(1);
    let state = AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
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
        server_signing_key: None,
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    }
}
