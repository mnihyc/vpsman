use super::*;

#[test]
fn webhook_rule_upsert_maps_name_conflicts_only() {
    let conflict = webhook_rule_upsert_error(anyhow::anyhow!("webhook_rule_name_conflict"));
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(conflict.code, "webhook_rule_name_conflict");

    let unexpected = webhook_rule_upsert_error(anyhow::anyhow!("storage_unavailable"));
    assert_eq!(unexpected.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(unexpected.code, "internal_server_error");
}

#[test]
fn webhook_rule_target_errors_keep_the_specific_validation_reason() {
    let request = CreateWebhookRuleRequest {
        id: None,
        name: "Operations".to_string(),
        enabled: true,
        expression: "interval.30sec".to_string(),
        target: "not-a-webhook-url".to_string(),
        body_template: "{event.kind}".to_string(),
        signing_secret: None,
        clear_signing_secret: false,
        cooldown_secs: Some(300),
        notes: None,
        confirmed: true,
    };
    let error = validate_webhook_rule_request(&request)
        .expect_err("an invalid target must be rejected before persistence");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "webhook_rule_target_invalid");
    assert!(
        error
            .public_message
            .as_deref()
            .is_some_and(|message| message.contains("absolute URL")),
        "the API should preserve the actionable URL validation reason"
    );
}

#[test]
fn webhook_rule_template_errors_keep_the_parser_reason() {
    let request = CreateWebhookRuleRequest {
        id: None,
        name: "Operations".to_string(),
        enabled: true,
        expression: "interval.30sec".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: "[if alert.severity =]invalid[endif]".to_string(),
        signing_secret: None,
        clear_signing_secret: false,
        cooldown_secs: Some(300),
        notes: None,
        confirmed: true,
    };
    let error = validate_webhook_rule_request(&request)
        .expect_err("invalid template syntax must be rejected before persistence");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "webhook_rule_template_invalid");
    assert!(
        error
            .public_message
            .as_deref()
            .is_some_and(|message| !message.trim().is_empty()),
        "the API should preserve the actionable template parser reason"
    );
}
