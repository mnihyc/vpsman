use super::*;

#[test]
fn chart_step_covers_inclusive_multi_day_endpoints() {
    let range = DashboardRange {
        mode: "all",
        window: None,
        start_unix: 0,
        end_unix: 4 * 24 * 60 * 60,
    };

    assert_eq!(
        dashboard_chart_step_secs(&range, 2),
        range.end_unix - range.start_unix
    );
}

#[test]
fn overview_singleflight_range_key_normalizes_relative_time() {
    let query = dashboard_query(Some("1d"), None, None);
    let first = prepare_dashboard_overview(&query, 2_000_000_000).unwrap();
    let second = prepare_dashboard_overview(&query, 2_000_000_001).unwrap();

    assert_eq!(
        dashboard_overview_range_singleflight_key(&query, &first),
        dashboard_overview_range_singleflight_key(&query, &second)
    );
}

#[test]
fn overview_singleflight_range_key_separates_custom_bounds() {
    let first_query = dashboard_query(None, Some(1_900_000_000), Some(1_900_000_100));
    let second_query = dashboard_query(None, Some(1_900_000_001), Some(1_900_000_100));
    let first = prepare_dashboard_overview(&first_query, 2_000_000_000).unwrap();
    let second = prepare_dashboard_overview(&second_query, 2_000_000_001).unwrap();

    assert_ne!(
        dashboard_overview_range_singleflight_key(&first_query, &first),
        dashboard_overview_range_singleflight_key(&second_query, &second)
    );
}

#[test]
fn overview_singleflight_range_key_separates_future_explicit_ends_after_clamping() {
    let now = 2_000_000_000;
    let near = dashboard_query(None, Some(now - 100), Some(now + 5));
    let far = dashboard_query(None, Some(now - 100), Some(now + 100));
    let near_prepared = prepare_dashboard_overview(&near, now).unwrap();
    let far_prepared = prepare_dashboard_overview(&far, now).unwrap();

    assert_eq!(near_prepared.range.end_unix, now);
    assert_eq!(far_prepared.range.end_unix, now);
    assert_ne!(
        dashboard_overview_range_singleflight_key(&near, &near_prepared),
        dashboard_overview_range_singleflight_key(&far, &far_prepared),
    );
}

fn dashboard_query(
    window: Option<&str>,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
) -> DashboardOverviewQuery {
    DashboardOverviewQuery {
        window: window.map(str::to_string),
        start_unix,
        end_unix,
        start_at: None,
        end_at: None,
        scope_kind: None,
        scope_value: None,
        group_by: None,
        resource_metric: None,
        chart_points: None,
    }
}
