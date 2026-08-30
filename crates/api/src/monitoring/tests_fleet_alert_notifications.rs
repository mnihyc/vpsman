use super::*;

#[test]
fn severity_threshold_matches_more_severe_alerts() {
    assert!(severity_rank("critical") <= severity_rank("warning"));
    assert!(severity_rank("warning") <= severity_rank("warning"));
    assert!(severity_rank("info") > severity_rank("warning"));
}

#[test]
fn delivery_error_keeps_nested_transport_cause_and_is_bounded() {
    let error = anyhow::anyhow!("connection refused").context("webhook request failed");
    assert_eq!(
        format_delivery_error(&error),
        "webhook request failed: connection refused"
    );
    let long = anyhow::anyhow!("x".repeat(MAX_NOTIFICATION_ERROR_BYTES + 100));
    assert_eq!(
        format_delivery_error(&long).len(),
        MAX_NOTIFICATION_ERROR_BYTES
    );
}

#[test]
fn process_lease_covers_one_exact_delivery_timeout() {
    assert_eq!(notification_delivery_lease_secs(), 65);
}

#[test]
fn reviewed_process_path_uses_the_shared_durable_owner_around_http() {
    let source = include_str!("fleet_alert_notifications.rs");
    let (_, process) = source
        .split_once("pub(crate) async fn process_fleet_alert_notifications")
        .expect("reviewed alert-notification process path");
    let (process, _) = process
        .split_once("async fn fleet_alert_delivery_actor_authorized")
        .expect("reviewed alert-notification process boundary");
    let claim = process
        .find("claim_fleet_alert_notification_delivery_for_process")
        .expect("exact reviewed delivery claim");
    let revalidate = process
        .find("begin_fleet_alert_notification_send")
        .expect("pre-I/O eligibility revalidation");
    let http = process
        .find("deliver_notification(&delivery).await")
        .expect("notification HTTP boundary");
    let completion = process
        .find("complete_fleet_alert_notification_delivery_attempt")
        .expect("token-fenced completion");
    assert!(claim < revalidate && revalidate < http && http < completion);
    assert!(process.contains("for requested_delivery in filtered_deliveries"));
    assert!(process.contains("delivery_id,"));
    assert!(process.contains("let lease_id = Uuid::new_v4();"));
    assert!(!process.contains("pool.begin"));
    assert!(!process.contains("Transaction<'_"));

    let repository = include_str!("../repository/fleet/repository_alert_notifications.rs");
    let (_, claim_sql) = repository
        .split_once("pub(crate) async fn claim_fleet_alert_notification_delivery_for_process")
        .expect("shared notification claim SQL");
    let (claim_sql, _) = claim_sql
        .split_once("pub(crate) async fn fleet_alert_notification_channel_enabled")
        .expect("shared notification claim boundary");
    assert!(claim_sql.contains("WHERE delivery.id = $1"));
    assert!(claim_sql.contains("FOR UPDATE OF delivery SKIP LOCKED"));
    assert!(claim_sql.contains("delivery_lease_id = $2"));
    assert!(claim_sql.contains(".fetch_optional(pool)"));
    assert!(!claim_sql.contains("unnest("));

    let (_, completion_sql) = repository
        .split_once("async fn postgres_complete_fleet_alert_notification_delivery_attempt")
        .expect("shared notification completion SQL");
    let (completion_sql, _) = completion_sql
        .split_once("async fn postgres_cancel_claimed_fleet_alert_notification_delivery")
        .expect("shared notification completion boundary");
    assert!(completion_sql.contains("status = 'in_progress'"));
    assert!(completion_sql.contains("delivery_lease_id = $2"));
    assert!(completion_sql.contains("eligibility_revision=$5"));
}

#[test]
fn reviewed_process_claim_race_is_an_ordered_skip_and_owned_rows_are_audited_immediately() {
    let source = include_str!("fleet_alert_notifications.rs");
    let (_, process) = source
        .split_once("pub(crate) async fn process_fleet_alert_notifications")
        .expect("reviewed alert-notification process path");
    let (process, _) = process
        .split_once("async fn fleet_alert_delivery_actor_authorized")
        .expect("reviewed alert-notification process boundary");

    let loop_start = process
        .find("for requested_delivery in filtered_deliveries")
        .expect("reviewed order loop");
    let exact_claim = process
        .find("let Some(delivery) = self")
        .expect("fallible exact claim");
    let skipped = process
        .find("NOTIFICATION_PROCESS_OUTCOME_SKIPPED_CURRENT_OWNER")
        .expect("explicit current-owner outcome");
    let first_continue = process[skipped..]
        .find("continue;")
        .map(|offset| skipped + offset)
        .expect("claim-race continuation");
    let completion = process
        .find("complete_fleet_alert_notification_delivery_attempt")
        .expect("owned-row completion");
    let completion_audit = process[completion..]
        .find("record_fleet_alert_notification_process_audit")
        .map(|offset| completion + offset)
        .expect("immediate owned-row audit");
    let completion_push = process[completion_audit..]
        .find("processed.push(completion)")
        .map(|offset| completion_audit + offset)
        .expect("owned-row response append");

    assert!(loop_start < exact_claim && exact_claim < skipped && skipped < first_continue);
    assert!(completion < completion_audit && completion_audit < completion_push);
    assert!(
        process
            .matches("record_fleet_alert_notification_process_audit")
            .count()
            >= 3
    );
    assert!(process[exact_claim..first_continue].contains("processed.push(skipped)"));
    assert!(!process[exact_claim..first_continue].contains(".ok_or_else"));
    assert!(!process.contains("claim_fleet_alert_notification_deliveries_for_process"));
}
