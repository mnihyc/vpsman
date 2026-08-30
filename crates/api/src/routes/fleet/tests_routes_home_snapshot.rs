use super::*;

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
