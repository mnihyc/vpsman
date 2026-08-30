use super::{
    automatic_evidence_ready, compare_optional_timestamps_desc, current_reachability_windows,
    update_plan_status,
};
use crate::model::{NetworkObservationView, NetworkOspfRecommendationView};
use std::cmp::Ordering;
use uuid::Uuid;
use vpsman_common::OspfControlMode;

#[test]
fn recommendation_ordering_handles_mixed_timestamp_formats_and_missing_evidence() {
    assert_eq!(
        compare_optional_timestamps_desc(Some("1770000000"), Some("2026-01-01T00:00:00Z"),),
        Ordering::Less,
    );
    assert_eq!(
        compare_optional_timestamps_desc(Some("1770000000"), None),
        Ordering::Less,
    );
}

#[test]
fn reachability_streak_requires_healthy_bilateral_windows() {
    let now = 1_800_000_000;
    let mut observations = Vec::new();
    for window in 0..3 {
        let observed = now - 10 - window * 60;
        observations.push(reachability_observation("left", observed, 60, true));
        observations.push(reachability_observation("right", observed - 5, 60, true));
    }
    let windows = current_reachability_windows(observations.iter(), now);
    assert_eq!(windows.len(), 3);
    assert_eq!(
        windows
            .iter()
            .take_while(|window| window.is_healthy())
            .count(),
        3
    );

    observations.retain(|observation| observation.endpoint_side.as_deref() == Some("left"));
    assert!(current_reachability_windows(observations.iter(), now).is_empty());
}

#[test]
fn endpoint_phase_shift_keeps_the_last_complete_windows() {
    let now = 1_800_000_000;
    let observations = [
        reachability_observation("left", now, 60, true),
        reachability_observation("left", now - 60, 60, true),
        reachability_observation("left", now - 120, 60, true),
        reachability_observation("left", now - 180, 60, true),
        reachability_observation("right", now - 40, 60, true),
        reachability_observation("right", now - 100, 60, true),
        reachability_observation("right", now - 160, 60, true),
    ];

    let windows = current_reachability_windows(observations.iter(), now);
    assert_eq!(windows.len(), 3);
    assert!(windows.iter().all(|window| window.is_healthy()));
}

#[test]
fn hourly_reachability_can_satisfy_the_ten_window_bound() {
    let now = 1_800_000_000;
    let mut observations = Vec::new();
    for window in 0..10 {
        let observed = now - 300 - window * 3_600;
        observations.push(reachability_observation("left", observed, 3_600, true));
        observations.push(reachability_observation(
            "right",
            observed - 120,
            3_600,
            true,
        ));
    }

    let windows = current_reachability_windows(observations.iter(), now);
    assert_eq!(windows.len(), 10);
    assert!(windows.iter().all(|window| window.is_healthy()));
    assert!(
        now - observations
            .last()
            .unwrap()
            .observed_at
            .parse::<i64>()
            .unwrap()
            > 10_800
    );
}

#[test]
fn ospf_eligibility_keeps_endpoint_loss_and_initial_cost_gates_distinct() {
    let mut recommendation = ospf_recommendation();
    assert_eq!(
        update_plan_status(
            &recommendation,
            true,
            false,
            true,
            "verified",
            20,
            OspfControlMode::Reviewed,
            5,
            2,
            2,
        ),
        "needs_adapter_status"
    );

    recommendation.confidence = "no_recent_observations".to_string();
    recommendation.latency_avg_ms = None;
    recommendation.latest_observed_at = None;
    assert_eq!(
        update_plan_status(
            &recommendation,
            true,
            true,
            false,
            "verified",
            0,
            OspfControlMode::Reviewed,
            5,
            2,
            0,
        ),
        "review_planned_baseline"
    );

    recommendation.confidence = "latency_only".to_string();
    recommendation.latency_avg_ms = Some(12.0);
    recommendation.latest_observed_at = Some("2026-08-25T00:00:00Z".to_string());
    recommendation.degraded_count = 1;
    assert_eq!(
        update_plan_status(
            &recommendation,
            true,
            true,
            true,
            "verified",
            20,
            OspfControlMode::Reviewed,
            5,
            2,
            0,
        ),
        "review_degraded"
    );

    recommendation.degraded_count = 0;
    assert!(!automatic_evidence_ready(&recommendation, 2, 2));
    recommendation.packet_loss_avg_ratio = Some(0.0);
    assert!(automatic_evidence_ready(&recommendation, 2, 2));
}

fn ospf_recommendation() -> NetworkOspfRecommendationView {
    NetworkOspfRecommendationView {
        recommendation_id: "ospf-test".to_string(),
        plan_id: Uuid::nil(),
        plan_name: "ospf-test".to_string(),
        interface_name: "tun-test".to_string(),
        left_client_id: "left".to_string(),
        right_client_id: "right".to_string(),
        configured_bandwidth_mbps: 100,
        effective_bandwidth_mbps: 100,
        plan_ospf_cost: 10,
        recommended_ospf_cost: 30,
        cost_delta: 20,
        latency_avg_ms: Some(12.0),
        packet_loss_avg_ratio: None,
        throughput_avg_mbps: None,
        throughput_max_mbps: None,
        sample_count: 2,
        degraded_count: 0,
        latest_observed_at: Some("2026-08-25T00:00:00Z".to_string()),
        confidence: "latency_only".to_string(),
        reason: "test".to_string(),
        evidence_summary: "test".to_string(),
    }
}

fn reachability_observation(
    endpoint_side: &str,
    observed_unix: i64,
    interval_secs: i64,
    healthy: bool,
) -> NetworkObservationView {
    NetworkObservationView {
        id: Uuid::new_v4(),
        job_id: None,
        client_id: format!("{endpoint_side}-client"),
        seq: None,
        kind: "tunnel_reachability".to_string(),
        source: "automatic".to_string(),
        role: Some("endpoint".to_string()),
        plan_id: Some(Uuid::nil()),
        topology_identity_hash: Some("a".repeat(64)),
        plan_name: Some("left-right".to_string()),
        interface_name: Some("tunlr".to_string()),
        peer_client_id: Some(format!("{endpoint_side}-peer")),
        target: Some("192.0.2.1".to_string()),
        endpoint_side: Some(endpoint_side.to_string()),
        address_family: Some("ipv4".to_string()),
        stale_after_secs: Some((interval_secs * 3).max(180)),
        healthy: Some(healthy),
        transmitted: Some(3),
        received: Some(if healthy { 3 } else { 0 }),
        latency_min_ms: healthy.then_some(10.0),
        latency_avg_ms: healthy.then_some(12.0),
        latency_max_ms: healthy.then_some(14.0),
        latency_mdev_ms: healthy.then_some(1.0),
        packet_loss_ratio: Some(if healthy { 0.0 } else { 1.0 }),
        reason: (!healthy).then(|| "probe_failed".to_string()),
        throughput_mbps: None,
        bytes: None,
        metadata: serde_json::json!({}),
        observed_at: observed_unix.to_string(),
        received_at: observed_unix.to_string(),
    }
}
