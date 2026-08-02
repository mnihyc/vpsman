use super::{validate_window, window_seconds};

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
