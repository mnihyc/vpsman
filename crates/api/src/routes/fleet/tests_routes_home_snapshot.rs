use super::*;

#[test]
fn home_history_projection_preserves_default_and_dashboard_query() {
    let default = Query::<HomeSnapshotQuery>::try_from_uri(
        &"/api/v1/home/snapshot?window=1d&chart_points=240"
            .parse()
            .unwrap(),
    )
    .unwrap();
    assert!(default.include_system_history.unwrap_or(true));

    let uri = "/api/v1/home/snapshot?window=7d&chart_points=480&include_system_history=false"
        .parse()
        .unwrap();
    let current = Query::<HomeSnapshotQuery>::try_from_uri(&uri).unwrap();
    assert_eq!(current.include_system_history, Some(false));
    let dashboard = Query::<DashboardOverviewQuery>::try_from_uri(&uri).unwrap();
    assert_eq!(dashboard.window.as_deref(), Some("7d"));
    assert_eq!(dashboard.chart_points, Some(480));
}

#[test]
fn home_history_projections_cannot_reuse_each_others_cached_response() {
    let operator = crate::model::OperatorView {
        id: uuid::Uuid::new_v4(),
        username: "operator".to_string(),
        status: "active".to_string(),
        role: "admin".to_string(),
        scopes: vec![SCOPE_FLEET_READ.to_string()],
        preferences: Default::default(),
        totp_enabled: false,
        session_refresh_ttl_secs: 0,
        created_at: "2026-09-07T00:00:00Z".to_string(),
        disabled_at: None,
        deleted_at: None,
    };
    let query = Query::<DashboardOverviewQuery>::try_from_uri(
        &"/api/v1/home/snapshot?window=1d".parse().unwrap(),
    )
    .unwrap();
    let prepared = prepare_dashboard_overview(&query, 1_788_710_400).unwrap();
    assert_ne!(
        home_snapshot_singleflight_key(&operator, &query, Some(&prepared), true),
        home_snapshot_singleflight_key(&operator, &query, Some(&prepared), false),
    );
}

#[tokio::test]
async fn home_snapshot_source_preserves_partial_failure_isolation() {
    let available = load_source("available", true, async {
        Ok::<_, anyhow::Error>(vec![1, 2])
    })
    .await;
    assert_eq!(available.data, Some(vec![1, 2]));
    assert_eq!(available.error, None);

    let unavailable = load_source("failed", true, async {
        anyhow::bail!("fixture source failed")
    })
    .await;
    assert_eq!(unavailable.data, None::<Vec<i32>>);
    assert_eq!(
        unavailable.error.as_deref(),
        Some("home_snapshot_failed_unavailable")
    );
}
