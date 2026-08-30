#[test]
fn topology_graph_errors_identify_the_failed_safe_stage() {
    use crate::repository_topology_graph::TopologyGraphStageError;

    for (stage, expected_code) in [
        (
            TopologyGraphStageError::Agents,
            "topology_graph_agents_unavailable",
        ),
        (
            TopologyGraphStageError::Plans,
            "topology_graph_plans_unavailable",
        ),
        (
            TopologyGraphStageError::Observations,
            "topology_graph_observations_unavailable",
        ),
        (
            TopologyGraphStageError::RuntimeTelemetry,
            "topology_graph_runtime_telemetry_unavailable",
        ),
        (
            TopologyGraphStageError::OspfRecommendations,
            "topology_graph_ospf_recommendations_unavailable",
        ),
        (
            TopologyGraphStageError::Contract,
            "topology_graph_contract_invalid",
        ),
    ] {
        let error = anyhow::anyhow!("private database detail").context(stage);
        let mapped = crate::routes_network::topology_graph_error(error);
        assert_eq!(mapped.code, expected_code);
        assert_eq!(mapped.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(mapped
            .public_message
            .as_deref()
            .is_some_and(|message| !message.contains("database")));
        assert_eq!(
            mapped.error.downcast_ref::<TopologyGraphStageError>(),
            Some(&stage)
        );
    }
}
