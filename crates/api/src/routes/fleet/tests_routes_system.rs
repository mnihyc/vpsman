use super::{
    gateway_events_view, requested_chart_step_secs, retained_system_resolution_for_age,
    system_metric_label_unit, tier_aligned_system_step_secs, validate_window, window_seconds,
};
use crate::{
    model::{
        SystemDashboardCancellationsView, SystemDashboardDbPoolView, SystemDashboardDispatchView,
        SystemDashboardGatewayEventsView, SystemDashboardTargetsView,
    },
    repository_system_dashboard::{
        system_metric_samples_from_snapshot, SystemDashboardRepositorySnapshot,
    },
};
use vpsman_common::GatewayForwardMetricsSnapshot;

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

#[test]
fn system_dashboard_projects_and_labels_gateway_telemetry_admission() {
    let projected = gateway_events_view(GatewayForwardMetricsSnapshot {
        telemetry_admission_limit: 8,
        telemetry_admission_active: 5,
        telemetry_admission_waiting: 3,
        ..GatewayForwardMetricsSnapshot::default()
    });
    assert_eq!(projected.telemetry_admission_limit, Some(8));
    assert_eq!(projected.telemetry_admission_active, Some(5));
    assert_eq!(projected.telemetry_admission_waiting, Some(3));

    assert_eq!(
        system_metric_label_unit("gateway_events.telemetry_admission_limit"),
        ("Gateway telemetry admission limit", "posts")
    );
    assert_eq!(
        system_metric_label_unit("gateway_events.telemetry_admission_active"),
        ("Gateway telemetry posts active", "posts")
    );
    assert_eq!(
        system_metric_label_unit("gateway_events.telemetry_admission_waiting"),
        ("Gateway telemetry posts waiting", "posts")
    );
}

#[test]
fn system_dashboard_samples_gateway_telemetry_admission_history() {
    let snapshot = SystemDashboardRepositorySnapshot {
        db_pool: SystemDashboardDbPoolView {
            max_connections: 0,
            open_connections: 0,
            idle_connections: 0,
            in_use_connections: 0,
        },
        dispatch: SystemDashboardDispatchView::default(),
        targets: SystemDashboardTargetsView::default(),
        cancellations: SystemDashboardCancellationsView::default(),
    };
    let gateway_events = SystemDashboardGatewayEventsView {
        telemetry_admission_limit: Some(8),
        telemetry_admission_active: Some(6),
        telemetry_admission_waiting: Some(4),
        status: "live".to_string(),
        ..SystemDashboardGatewayEventsView::default()
    };
    let samples = system_metric_samples_from_snapshot(&snapshot, &gateway_events);

    for (metric, expected) in [
        ("gateway_events.telemetry_admission_limit", 8.0),
        ("gateway_events.telemetry_admission_active", 6.0),
        ("gateway_events.telemetry_admission_waiting", 4.0),
    ] {
        assert_eq!(
            samples
                .iter()
                .find(|sample| sample.metric == metric)
                .map(|sample| sample.value),
            Some(expected),
            "missing or incorrect sample for {metric}"
        );
    }
}
