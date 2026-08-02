use super::*;

#[test]
fn controller_plan_error_is_isolated_from_following_plan() {
    let failed_plan_id = Uuid::new_v4();
    let healthy_plan_id = Uuid::new_v4();

    let failed = isolate_controller_plan_result::<usize>(
        failed_plan_id,
        "automatic_update",
        Err(anyhow::anyhow!("poison plan")),
    );
    let healthy = isolate_controller_plan_result(healthy_plan_id, "automatic_update", Ok(2_usize));

    assert_eq!(failed, None);
    assert_eq!(healthy, Some(2));
}

#[test]
fn automatic_status_refreshes_initial_stale_failed_and_periodic_verified_states() {
    let now = Utc::now();
    let recent = (now - Duration::seconds(60)).to_rfc3339();
    let retry_due = (now - Duration::seconds(FAILED_STATUS_RETRY_AFTER_SECS)).to_rfc3339();
    let refresh_due = (now - Duration::seconds(VERIFIED_STATUS_REFRESH_AFTER_SECS)).to_rfc3339();

    assert!(automatic_status_refresh_due(
        "unverified",
        "verified",
        &recent,
        now
    ));
    assert!(!automatic_status_refresh_due(
        "failed", "verified", &recent, now
    ));
    assert!(automatic_status_refresh_due(
        "failed", "verified", &retry_due, now
    ));
    assert!(!automatic_status_refresh_due(
        "verified", "verified", &recent, now
    ));
    assert!(automatic_status_refresh_due(
        "verified",
        "verified",
        &refresh_due,
        now
    ));
    assert!(!automatic_status_refresh_due(
        "pending",
        "verified",
        &refresh_due,
        now
    ));
}
