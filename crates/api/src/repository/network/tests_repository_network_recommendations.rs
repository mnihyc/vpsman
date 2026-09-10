use super::{
    automatic_evidence_ready, compare_optional_timestamps_desc, current_reachability_windows,
    recommend_plan_ospf_cost, topology_identity_hash_for_plan, update_plan_status,
};
use crate::model::{
    NetworkObservationView, NetworkOspfRecommendationView, TunnelPlanEndpointRuntimeConfigView,
    TunnelPlanView,
};
use std::cmp::Ordering;
use uuid::Uuid;
use vpsman_common::OspfControlMode;

#[test]
fn dynamic_bandwidth_is_opt_in_without_changing_evidence_or_reachability_gates() {
    for mode in [OspfControlMode::Reviewed, OspfControlMode::Automatic] {
        let mut plan = bandwidth_test_plan();
        plan.input.ospf.as_mut().unwrap().mode = mode;
        plan.plan.ospf.as_mut().unwrap().mode = mode;
        let now = chrono::Utc::now().timestamp();
        let mut observations = bandwidth_test_reachability(&plan, now);
        let baseline = recommend_plan_ospf_cost(&plan, &observations);
        observations.push(bandwidth_test_speed(&plan, now - 5, 20.0));
        observations.push(bandwidth_test_speed(&plan, now - 6, 60.0));

        let fixed = recommend_plan_ospf_cost(&plan, &observations);
        assert_eq!(fixed.view.effective_bandwidth_mbps, 1_000);
        assert_eq!(
            fixed.view.recommended_ospf_cost,
            baseline.view.recommended_ospf_cost
        );
        assert_eq!(fixed.view.confidence, "latency_only");
        assert!(fixed.view.reason.contains("dynamic bandwidth is disabled"));
        assert_eq!(fixed.view.throughput_avg_mbps, Some(40.0));
        assert_eq!(fixed.view.throughput_max_mbps, Some(60.0));

        plan.input.dynamic_bandwidth = true;
        let dynamic = recommend_plan_ospf_cost(&plan, &observations);
        assert_eq!(dynamic.view.effective_bandwidth_mbps, 40);
        assert!(dynamic.view.recommended_ospf_cost > fixed.view.recommended_ospf_cost);
        assert_eq!(dynamic.view.confidence, "measured");
        assert_eq!(
            dynamic.view.throughput_avg_mbps,
            fixed.view.throughput_avg_mbps
        );
        assert_eq!(dynamic.view.sample_count, fixed.view.sample_count);
        assert_eq!(dynamic.view.degraded_count, fixed.view.degraded_count);
        assert_eq!(
            dynamic.view.latest_observed_at,
            fixed.view.latest_observed_at
        );
        assert_eq!(dynamic.healthy_probe_streak, fixed.healthy_probe_streak);
        assert!(automatic_evidence_ready(
            &fixed.view,
            2,
            fixed.healthy_probe_streak
        ));
        assert!(automatic_evidence_ready(
            &dynamic.view,
            2,
            dynamic.healthy_probe_streak
        ));

        let speed_only = &observations[4..];
        let dynamic_without_probe = recommend_plan_ospf_cost(&plan, speed_only);
        assert_eq!(dynamic_without_probe.view.effective_bandwidth_mbps, 1_000);
        assert_eq!(
            dynamic_without_probe.view.recommended_ospf_cost,
            plan.recommended_ospf_cost.unwrap(),
        );
        assert!(!automatic_evidence_ready(&dynamic_without_probe.view, 2, 0));
        plan.input.dynamic_bandwidth = false;
        let fixed_without_probe = recommend_plan_ospf_cost(&plan, speed_only);
        assert_eq!(fixed_without_probe.view.confidence, "throughput_only");
        assert!(fixed_without_probe
            .view
            .reason
            .contains("available for inspection"));
        assert_eq!(
            fixed_without_probe.view.recommended_ospf_cost,
            dynamic_without_probe.view.recommended_ospf_cost,
        );
    }
}

#[test]
fn dynamic_bandwidth_keeps_expired_and_changed_topology_speed_evidence_out() {
    let mut plan = bandwidth_test_plan();
    plan.input.dynamic_bandwidth = true;
    let now = chrono::Utc::now().timestamp();
    let reachability = bandwidth_test_reachability(&plan, now);
    let baseline = recommend_plan_ospf_cost(&plan, &reachability);
    let expired = bandwidth_test_speed(&plan, now - 11 * 60, 20.0);
    let mut old_kind = bandwidth_test_speed(&plan, now - 5, 20.0);
    let mut old_plan = plan.clone();
    old_plan.plan.kind = vpsman_common::TunnelKind::Ipip;
    old_kind.topology_identity_hash = Some(topology_identity_hash_for_plan(&old_plan));

    for rejected in [expired, old_kind] {
        let mut observations = reachability.clone();
        observations.push(rejected);
        let fallback = recommend_plan_ospf_cost(&plan, &observations);
        assert_eq!(fallback.view.effective_bandwidth_mbps, 1_000);
        assert_eq!(
            fallback.view.recommended_ospf_cost,
            baseline.view.recommended_ospf_cost
        );
        assert_eq!(fallback.view.confidence, "latency_only");
        assert_eq!(fallback.view.throughput_avg_mbps, None);
        assert_eq!(fallback.view.sample_count, baseline.view.sample_count);
    }
}

fn bandwidth_test_plan() -> TunnelPlanView {
    let input: vpsman_common::TunnelPlanInput = serde_json::from_value(serde_json::json!({
        "name": "left-right",
        "interface_name": "tunlr",
        "kind": "gre",
        "left_client_id": "left-client",
        "right_client_id": "right-client",
        "left_remote_underlay": "192.0.2.2",
        "right_remote_underlay": "192.0.2.1",
        "address_pool_cidr": "10.0.0.0/24",
        "ipv4_tunnel": {"left": "10.0.0.0", "right": "10.0.0.1", "prefix_len": 31},
        "bandwidth_mbps": 1000,
        "left_mtu": 1476,
        "right_mtu": 1476,
        "ospf": {"planned_latency_ms": 20.0, "planned_packet_loss_ratio": 0.0, "preference": 1.0}
    }))
    .unwrap();
    let plan = vpsman_common::plan_tunnel(&input).unwrap();
    let endpoint = |client_id: &str| TunnelPlanEndpointRuntimeConfigView {
        client_id: client_id.to_string(),
        desired: "enabled".to_string(),
        status: "applied".to_string(),
        job_id: None,
        error: None,
        updated_at: None,
    };
    TunnelPlanView {
        id: Uuid::nil(),
        name: plan.name.clone(),
        kind: plan.kind,
        enabled: true,
        revision: 1,
        left_client_id: plan.left_client_id.clone(),
        right_client_id: plan.right_client_id.clone(),
        recommended_ospf_cost: plan.recommended_ospf_cost.map(i32::from),
        ospf_status: "verified".to_string(),
        left_ospf_status: "verified".to_string(),
        right_ospf_status: "verified".to_string(),
        desired_ospf_cost: None,
        left_current_ospf_cost: None,
        right_current_ospf_cost: None,
        left_ospf_job_id: None,
        right_ospf_job_id: None,
        connection_assessment: "unknown".to_string(),
        connection_assessment_note: None,
        connection_assessed_at: None,
        connection_assessed_by: None,
        left_runtime_config: endpoint(&plan.left_client_id),
        right_runtime_config: endpoint(&plan.right_client_id),
        input,
        plan,
        builtin_credentials: None,
        created_at: String::new(),
        updated_at: String::new(),
        deleted_at: None,
        deleted_by: None,
        deleted_reason: None,
    }
}

fn bandwidth_test_reachability(plan: &TunnelPlanView, now: i64) -> Vec<NetworkObservationView> {
    let mut observations = Vec::new();
    for offset in [10, 70] {
        for side in ["left", "right"] {
            let mut observation = reachability_observation(side, now - offset, 60, true);
            observation.topology_identity_hash = Some(topology_identity_hash_for_plan(plan));
            observations.push(observation);
        }
    }
    observations
}

fn bandwidth_test_speed(plan: &TunnelPlanView, observed: i64, mbps: f64) -> NetworkObservationView {
    let mut observation = reachability_observation("left", observed, 60, true);
    observation.kind = "network_speed_test".to_string();
    observation.source = "manual".to_string();
    observation.topology_identity_hash = Some(topology_identity_hash_for_plan(plan));
    observation.throughput_mbps = Some(mbps);
    observation
}

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
