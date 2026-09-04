use super::*;
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
