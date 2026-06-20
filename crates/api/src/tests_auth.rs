use super::*;
use std::collections::BTreeMap;

use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};

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
async fn operator_login_throttle_locks_username_and_success_clears_username_bucket() {
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

    let audit_json = serde_json::to_string(&repo.list_audit_logs(10).await.unwrap()).unwrap();
    assert!(audit_json.contains("operator_auth.login_after_failures"));
    assert!(audit_json.contains("operator_auth.lockout_created"));
    assert!(audit_json.contains("\"scope_kind\":\"username\""));
    assert!(!audit_json.contains("\"scope_kind\":\"ip\""));
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
    let state = memory_test_state();
    let peer = "203.0.113.21:44321"
        .parse::<std::net::SocketAddr>()
        .unwrap();

    for index in 0..8 {
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
        session_id: Uuid::new_v4(),
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
        routes_job_history::list_network_observations(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(HistoryQuery { limit: None }),
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
                target: Some("https://hooks.example/vpsman".to_string()),
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
        routes_inventory::list_data_source_presets(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(DataSourcePresetQuery { domain: None }),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_inventory::list_hot_config_rule_templates(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
        )
        .await,
    );
    assert_scope_forbidden(
        routes_inventory::render_data_source_hot_config(
            axum::extract::State(state.clone()),
            viewer_headers.clone(),
            axum::extract::Query(DataSourceHotConfigQuery {
                client_id: "client-a".to_string(),
            }),
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
            audit_headers,
            axum::extract::Query(ListQuery::default()),
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
        routes_inventory::list_data_source_presets(
            axum::extract::State(state.clone()),
            config_headers.clone(),
            axum::extract::Query(DataSourcePresetQuery { domain: None }),
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
            axum::extract::Query(HistoryQuery { limit: None }),
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
        session_id: Uuid::new_v4(),
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
        axum::extract::Path(session.session_id),
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
        session_id: Uuid::new_v4(),
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
        session_id: Uuid::new_v4(),
    };

    let preferences = OperatorPreferences {
        language: "en".to_string(),
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
                timeout_secs: None,
                force_unprivileged: false,
                privileged: false,
                privilege_assertion: None,
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
                target: "https://hooks.example/vpsman".to_string(),
                body_template: String::new(),
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
        session_id: Uuid::new_v4(),
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
