use super::*;

#[test]
fn webhook_manual_dispatch_binds_commit_to_the_reviewed_event_identity() {
    let request = |dry_run, confirmed, event_id: Option<&str>, preview_hash: Option<&str>| {
        WebhookRuleDispatchRequest {
            rule_id: None,
            event_kind: "interval.30sec".to_string(),
            event_id: event_id.map(str::to_string),
            limit: Some(50),
            dry_run: Some(dry_run),
            preview_hash: preview_hash.map(str::to_string),
            confirmed,
        }
    };

    validate_webhook_rule_dispatch_request(&request(true, false, None, None))
        .expect("a dry run owns generation of its review event identity");

    for (event_id, preview_hash, expected_code) in [
        (
            Some("reviewed-event"),
            None,
            "webhook_rule_dispatch_preview_hash_required",
        ),
        (
            None,
            Some("review-hash"),
            "webhook_rule_dispatch_event_id_required",
        ),
    ] {
        let error =
            validate_webhook_rule_dispatch_request(&request(false, true, event_id, preview_hash))
                .expect_err("commit must retain both identities from its dry-run review");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, expected_code);
    }

    validate_webhook_rule_dispatch_request(&request(
        false,
        true,
        Some("reviewed-event"),
        Some("review-hash"),
    ))
    .expect("the exact reviewed event identity and hash may proceed");
}

#[test]
fn webhook_rule_upsert_maps_name_conflicts_only() {
    let conflict = webhook_rule_upsert_error(anyhow::anyhow!("webhook_rule_name_conflict"));
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(conflict.code, "webhook_rule_name_conflict");

    let unexpected = webhook_rule_upsert_error(anyhow::anyhow!("storage_unavailable"));
    assert_eq!(unexpected.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(unexpected.code, "webhook_rule_upsert_failed");
    assert_eq!(
        unexpected.public_message.as_deref(),
        Some("The webhook rule could not be saved.")
    );
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

#[test]
fn webhook_rule_template_accepts_commented_alternatives() {
    let request = CreateWebhookRuleRequest {
        id: None,
        name: "Operations".to_string(),
        enabled: true,
        expression: "interval.30sec".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: concat!(
            "{#\n",
            "Alert: [{alert.severity}] {alert.title} on {vps.display_name}\n",
            "Threshold: {traffic.cycle_percent}% [if intentionally invalid]\n",
            "#}\n",
            "{event.kind}",
        )
        .to_string(),
        signing_secret: None,
        clear_signing_secret: false,
        cooldown_secs: Some(300),
        notes: None,
        confirmed: true,
    };

    validate_webhook_rule_request(&request)
        .expect("commented alternatives must not participate in template validation");
}
