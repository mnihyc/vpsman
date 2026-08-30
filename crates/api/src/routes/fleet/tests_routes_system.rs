use super::{
    normalize_system_dashboard_query, requested_chart_step_secs,
    retained_system_resolution_for_age, system_dashboard_singleflight_key, system_dashboard_start,
    tier_aligned_system_step_secs, validate_window, window_seconds, SystemDashboardQuery,
};

#[test]
fn system_dashboard_singleflight_key_uses_canonical_query_and_auth() {
    let operator_id = uuid::Uuid::new_v4();
    let scopes = vec!["fleet:read".to_string(), "jobs:read".to_string()];
    let default = SystemDashboardQuery {
        window: None,
        chart_points: None,
    };
    let explicit = SystemDashboardQuery {
        window: Some(" 1d ".to_string()),
        chart_points: Some(240),
    };
    let (default_window, default_points) = normalize_system_dashboard_query(&default).unwrap();
    let (explicit_window, explicit_points) = normalize_system_dashboard_query(&explicit).unwrap();
    assert_eq!(
        (default_window, default_points),
        (explicit_window, explicit_points)
    );
    assert_eq!(
        system_dashboard_singleflight_key(operator_id, &scopes, default_window, default_points,),
        system_dashboard_singleflight_key(
            operator_id,
            &["jobs:read".to_string(), "fleet:read".to_string()],
            explicit_window,
            explicit_points,
        )
    );

    let clamped_low = SystemDashboardQuery {
        window: Some("1d".to_string()),
        chart_points: Some(0),
    };
    let clamped_high = SystemDashboardQuery {
        window: Some("1d".to_string()),
        chart_points: Some(i64::MAX),
    };
    assert_eq!(normalize_system_dashboard_query(&clamped_low).unwrap().1, 1);
    assert_eq!(
        normalize_system_dashboard_query(&clamped_high).unwrap().1,
        1_440
    );

    assert_ne!(
        system_dashboard_singleflight_key(operator_id, &scopes, "1d", 240),
        system_dashboard_singleflight_key(operator_id, &scopes, "all", 240),
    );
    assert_ne!(
        system_dashboard_singleflight_key(operator_id, &scopes, "1d", 240),
        system_dashboard_singleflight_key(operator_id, &scopes, "1d", 241),
    );
    assert_ne!(
        system_dashboard_singleflight_key(operator_id, &scopes, "1d", 240),
        system_dashboard_singleflight_key(uuid::Uuid::new_v4(), &scopes, "1d", 240),
    );
}

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

    for (span, points, expected_step, expected_points) in [
        (7 * DAY, 420, 1_500, 404),
        (7 * DAY, 480, 1_500, 404),
        (365 * DAY, 420, 86_400, 366),
        (365 * DAY, 480, 64_800, 487),
    ] {
        let resolution = retained_system_resolution_for_age(span);
        let requested = requested_chart_step_secs(span, points);
        let step = tier_aligned_system_step_secs(span, requested, resolution, points as u64);
        assert_eq!(step, expected_step);
        assert_eq!(span / step as u64 + 1, expected_points);
        assert_eq!(step % resolution, 0);
        assert!(expected_points <= points as u64 + 12);
    }
}

#[test]
fn all_window_density_uses_the_retained_extent_instead_of_unix_epoch() {
    const DAY: u64 = 86_400;
    let now = 1_787_600_000;
    let retained_span = 3_650 * DAY;
    let retained_start = now - retained_span;
    assert_eq!(
        system_dashboard_start(now, "all", Some(retained_start)),
        retained_start
    );
    assert_eq!(system_dashboard_start(now, "all", None), now);
    assert_eq!(
        system_dashboard_start(now, "1d", Some(retained_start)),
        now - DAY
    );
    let requested = requested_chart_step_secs(retained_span, 240);
    let step = tier_aligned_system_step_secs(
        retained_span,
        requested,
        retained_system_resolution_for_age(retained_span),
        240,
    );

    assert_eq!(step % DAY as i32, 0);
    assert!((230..=252).contains(&(retained_span / step as u64 + 1)));
}
