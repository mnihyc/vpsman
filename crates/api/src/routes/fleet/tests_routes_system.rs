use super::{
    requested_chart_step_secs, retained_system_resolution_for_age, tier_aligned_system_step_secs,
    validate_window, window_seconds,
};

#[test]
fn system_dashboard_uses_the_complete_monitoring_window_model() {
    let expected = [
        ("15m", 15 * 60),
        ("1h", 60 * 60),
        ("8h", 8 * 60 * 60),
        ("1d", 24 * 60 * 60),
        ("7d", 7 * 24 * 60 * 60),
        ("30d", 30 * 24 * 60 * 60),
        ("90d", 90 * 24 * 60 * 60),
        ("180d", 180 * 24 * 60 * 60),
        ("1y", 365 * 24 * 60 * 60),
        ("all", u64::MAX),
    ];
    for (value, seconds) in expected {
        assert_eq!(validate_window(Some(value)).unwrap(), value);
        assert_eq!(window_seconds(value), seconds);
    }
    assert_eq!(validate_window(None).unwrap(), "1d");
    assert!(validate_window(Some("6h")).is_err());
    assert!(validate_window(Some("24h")).is_err());
    assert!(validate_window(Some("14d")).is_err());
}

#[test]
fn system_dashboard_uses_tier_aligned_truthful_resolution() {
    const DAY: u64 = 86_400;
    assert_eq!(retained_system_resolution_for_age(2 * DAY), 60);
    assert_eq!(retained_system_resolution_for_age(2 * DAY + 1), 300);
    assert_eq!(retained_system_resolution_for_age(31 * DAY + 1), 3_600);
    assert_eq!(retained_system_resolution_for_age(366 * DAY + 1), 86_400);

    let requested = requested_chart_step_secs(30 * DAY, 720);
    assert_eq!(requested, 3_660);
    assert_eq!(
        tier_aligned_system_step_secs(30 * DAY, requested, 1_800, 720),
        3_600
    );
    let requested = requested_chart_step_secs(365 * DAY, 720);
    assert_eq!(
        tier_aligned_system_step_secs(365 * DAY, requested, 21_600, 720),
        43_200
    );
}
