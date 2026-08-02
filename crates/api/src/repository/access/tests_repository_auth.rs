use super::*;

#[tokio::test]
async fn failed_login_for_known_username_is_not_attributed_to_that_operator() {
    let repo = Repository::Memory(MemoryState::default());
    let password = "admin-password-123";
    let operator_id = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap()
        .operator
        .id;

    let attempt = repo
        .login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: "wrong-password-123".to_string(),
                totp_code: None,
            },
            "203.0.113.71",
            Some("audit-test-browser"),
            &OperatorAuthThrottleConfig::default(),
        )
        .await
        .unwrap();
    assert!(matches!(attempt, OperatorLoginAttempt::InvalidCredentials));

    let audit = repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .into_iter()
        .find(|audit| audit.action == "operator_auth.login_failure")
        .expect("failed login audit");
    assert_eq!(audit.actor_id, None);
    assert_eq!(audit.target, "operator-login:admin");
    assert_eq!(audit.metadata["attempted_username"], "admin");
    assert_eq!(audit.metadata["reason"], "bad_password");
    assert!(audit.metadata.get("operator_id").is_none());
    assert!(audit.metadata.get("operator_username").is_none());
    assert!(!serde_json::to_string(&audit.metadata)
        .unwrap()
        .contains(&operator_id.to_string()));
}
