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
