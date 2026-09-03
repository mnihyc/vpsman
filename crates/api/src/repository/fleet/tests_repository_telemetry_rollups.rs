use super::*;

#[test]
fn bounded_dashboard_traffic_queue_tracks_physical_origin_dependencies() {
    let migration = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");
    let (_, queue) = migration
        .split_once("CREATE FUNCTION public.queue_telemetry_dashboard_traffic_coordinates(")
        .expect("traffic coordinate queue");
    let (queue, _) = queue
        .split_once("CREATE FUNCTION public.queue_telemetry_dashboard_network_membership_change(")
        .expect("traffic coordinate queue boundary");

    assert!(queue.contains("p_origin_kinds TEXT[]"));
    assert!(queue.contains("item.origin_kind IN ('live', 'vnstat_import')"));
    assert!(queue.contains("selected.native_bucket_secs AS source_bucket_secs"));
    assert!(queue.contains("ARRAY[3600, 10800, 21600, 86400]::INTEGER[]"));
    assert_eq!(
        queue
            .matches("JOIN public.traffic_counter_rollups rollup")
            .count(),
        1
    );
    for physical_key in [
        "rollup.client_id = selected.client_id",
        "rollup.source_kind = selected.source_kind",
        "rollup.interface = selected.interface",
        "rollup.origin_kind = selected.origin_kind",
        "rollup.bucket_secs = tier.bucket_secs",
        "rollup.bucket_start = to_timestamp(",
        "tier.bucket_secs > selected.native_bucket_secs",
    ] {
        assert!(
            queue.contains(physical_key),
            "missing physical key: {physical_key}"
        );
    }
    assert!(!queue.contains("requires_containing_tiers"));
    assert_eq!(
        migration
            .matches("PERFORM public.queue_telemetry_dashboard_traffic_coordinates(")
            .count(),
        6,
        "every traffic source-table producer must declare its dependency scope"
    );

    let (_, live_insert) = migration
        .split_once(
            "CREATE FUNCTION public.queue_telemetry_dashboard_traffic_samples_after_insert()",
        )
        .expect("live traffic insert producer");
    let (live_insert, _) = live_insert
        .split_once("$$;")
        .expect("live traffic insert producer boundary");
    assert!(live_insert.contains("telemetry_dashboard_traffic_origin_kind("));
    assert!(live_insert.contains("payload.origin_kinds"));
    assert!(live_insert.contains("successor.observed_at"));
    assert!(live_insert.contains(") successor ON NOT successor.usage_authoritative"));
    assert!(!live_insert.contains("traffic_counter_streams"));
    assert!(!live_insert.contains("promoted_boundary_safe"));
    assert!(!live_insert.contains("source_revision"));

    for physical_producer in [
        "queue_telemetry_dashboard_traffic_samples_after_delete",
        "queue_telemetry_dashboard_traffic_samples_after_update",
        "queue_telemetry_dashboard_traffic_rollups_after_insert",
        "queue_telemetry_dashboard_traffic_rollups_after_delete",
        "queue_telemetry_dashboard_traffic_rollups_after_update",
    ] {
        let (_, body) = migration
            .split_once(&format!("CREATE FUNCTION public.{physical_producer}()"))
            .unwrap_or_else(|| panic!("missing {physical_producer}"));
        let (body, _) = body
            .split_once("$$;")
            .unwrap_or_else(|| panic!("missing {physical_producer} boundary"));
        assert!(body.contains("origin_kind"), "{physical_producer}");
        assert!(!body.contains("ARRAY[]::BOOLEAN[]"), "{physical_producer}");
    }

    assert!(migration.contains("CREATE FUNCTION public.telemetry_dashboard_traffic_origin_kind("));
    assert!(migration.contains(
        "public.telemetry_dashboard_traffic_origin_kind(\n                    exact.sample_source"
    ));
}

#[test]
fn bounded_dashboard_coordinate_replacement_owns_one_assembled_upsert_or_delete() {
    let migration = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");

    for (domain, source_probe, evidence_probe) in [
        (
            "resource",
            "LEFT JOIN public.telemetry_rollups source",
            "FROM unnest(assembled.sample_counts)",
        ),
        (
            "network",
            "public.telemetry_network_durable_points_source(",
            "FROM unnest(assembled.sample_counts)",
        ),
        (
            "traffic",
            "public.telemetry_dashboard_traffic_source_points(",
            "FROM unnest(assembled.rx_valid_counts)",
        ),
    ] {
        let marker =
            format!("CREATE FUNCTION public.replace_telemetry_dashboard_{domain}_coordinates(");
        let (_, body) = migration
            .split_once(&marker)
            .unwrap_or_else(|| panic!("missing {domain} coordinate replacement"));
        let (body, _) = body
            .split_once("$$;")
            .unwrap_or_else(|| panic!("missing {domain} coordinate replacement boundary"));
        assert!(body.contains("assembled AS MATERIALIZED"), "{domain}");
        assert!(body.contains("replacement AS MATERIALIZED"), "{domain}");
        assert_eq!(body.matches(source_probe).count(), 1, "{domain}");
        assert!(body.contains(evidence_probe), "{domain}");
        assert!(body.contains(&format!(
            "MERGE INTO public.telemetry_dashboard_{domain}_blocks AS target"
        )));
        assert!(body.contains("WHEN MATCHED AND NOT source.has_samples THEN\n        DELETE"));
        assert!(body.contains("WHEN NOT MATCHED AND source.has_samples THEN\n        INSERT"));
        assert!(!body.contains("ON CONFLICT"), "{domain}");
        assert!(!body.contains(&format!(
            "DELETE FROM public.telemetry_dashboard_{domain}_blocks"
        )));
    }
}

#[test]
fn live_dashboard_network_queue_only_requests_membership_generation() {
    let migration = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");
    let (_, queue) = migration
        .split_once("CREATE FUNCTION public.queue_telemetry_dashboard_network_membership_change(")
        .expect("live network dashboard membership owner");
    let (queue, _) = queue
        .split_once("CREATE FUNCTION public.queue_telemetry_dashboard_network_membership_removal(")
        .expect("active network dashboard queue boundary");

    assert!(queue.contains("telemetry_dashboard_effective_network_selection(p_client_id)"));
    assert!(queue.contains("telemetry_dashboard_network_interface_selected("));
    assert!(queue.contains("INTO selected_interfaces"));
    assert!(queue.contains("cardinality(selected_interfaces) = 0"));
    assert!(queue.contains("NOT selected_interfaces <@ head_interfaces"));
    let resident_receipt = queue
        .find("IF p_interfaces <@ head_interfaces")
        .expect("resident membership receipt");
    let selector_evaluation = queue
        .find("telemetry_dashboard_effective_network_selection(p_client_id)")
        .expect("network selector evaluation");
    assert!(
        resident_receipt < selector_evaluation,
        "existing resident membership must bypass per-interface policy evaluation"
    );
    assert_eq!(
        queue
            .matches("queue_telemetry_dashboard_generation(")
            .count(),
        1
    );
    assert!(!queue.contains("queue_telemetry_dashboard_coordinate("));
    assert!(!queue.contains("p_source_bucket_secs"));
    assert!(!queue.contains("p_bucket_start"));
    assert!(!queue.contains("head_select_all"));
}

#[test]
fn retained_network_producers_use_exact_membership_handoffs() {
    let migration = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");
    let (_, removal) = migration
        .split_once("CREATE FUNCTION public.queue_telemetry_dashboard_network_membership_removal(")
        .expect("network membership removal owner");
    let (removal, _) = removal
        .split_once("CREATE FUNCTION public.initialize_telemetry_dashboard_client()")
        .expect("network membership removal boundary");
    assert!(removal.contains("retained.client_id = p_client_id"));
    assert!(removal.contains("retained.interface = changed.interface"));
    assert!(removal.contains("stream.first_unpromoted_observed_at IS NOT NULL"));
    assert!(removal.contains("projected_suffix_interfaces AS MATERIALIZED"));
    assert!(!removal.contains("telemetry_dashboard_generation_interfaces("));

    for producer in [
        "queue_telemetry_network_blocks_after_insert",
        "queue_telemetry_network_blocks_after_delete",
        "queue_telemetry_network_blocks_after_update",
        "queue_telemetry_network_samples_after_insert",
        "queue_telemetry_network_samples_after_delete",
        "queue_telemetry_network_samples_after_update",
    ] {
        let (_, body) = migration
            .split_once(&format!("CREATE FUNCTION public.{producer}()"))
            .unwrap_or_else(|| panic!("missing {producer}"));
        let (body, _) = body
            .split_once("$$;")
            .unwrap_or_else(|| panic!("missing {producer} boundary"));
        assert!(!body.contains("telemetry_dashboard_generation_interfaces("));
        assert!(!body.contains("refresh_telemetry_dashboard_network_selection("));
    }

    let (_, live_insert) = migration
        .split_once("CREATE FUNCTION public.queue_telemetry_network_samples_after_insert()")
        .expect("live network minute producer");
    let (live_insert, _) = live_insert
        .split_once("$$;")
        .expect("live network minute producer boundary");
    assert!(!live_insert.contains("FOR affected IN"));
    assert!(live_insert.contains("novel AS MATERIALIZED"));
    assert!(live_insert.contains("resolve_telemetry_interface_policies(ARRAY("));
    assert!(live_insert.contains("telemetry_dashboard_network_interface_selected_resolved("));
}

#[test]
fn fleet_live_current_network_membership_is_decoded_once_per_online_client() {
    let source = include_str!("repository_telemetry_rollups.rs");
    let (_, current_query) = source
        .split_once("WITH online_current_interfaces AS MATERIALIZED")
        .expect("current network membership owner");
    let (current_query, _) = current_query
        .split_once("// Explicit physical-tier inspection")
        .expect("current network query boundary");

    assert_eq!(current_query.matches("jsonb_array_elements(").count(), 1);
    assert!(current_query.contains("GROUP BY projection.client_id"));
    assert!(current_query.contains("online_client.status = 'online'"));
    assert!(current_query.contains("client.status <> 'online'"));
    assert!(current_query.contains("network_current.interface = ANY("));
    assert!(current_query.contains("JOIN telemetry_projection_heads projection"));
    assert!(current_query.contains("$5::BOOLEAN"));
    assert!(current_query.contains("projection.client_id = ANY($6::TEXT[])"));
    assert!(current_query.contains("projection.client_id = ANY($7::TEXT[])"));
    assert!(current_query.contains("WITH ORDINALITY AS projected_network(value, ordinality)"));
    assert_eq!(
        current_query
            .matches("public.telemetry_ordinal_admission_mask_is_exact(")
            .count(),
        1,
        "current membership must reject the entire projected vector when its mask is malformed"
    );
    assert!(current_query.contains("latest.network_admission_mask"));
    assert!(current_query.contains("WHEN NOT public.telemetry_ordinal_admission_mask_is_exact("));
    assert!(!current_query.contains("$9"));
    assert!(current_query.contains("get_bit("));
}

#[test]
fn operational_tunnel_reader_uses_the_canonical_current_plan_identity() {
    let source = include_str!("repository_telemetry_rollups.rs");
    let (_, tunnel_reader) = source
        .split_once("async fn list_telemetry_tunnels_matching")
        .expect("operational tunnel reader");
    let (tunnel_reader, _) = tunnel_reader
        .split_once("pub(crate) fn tunnel_adapter_health_is_degraded")
        .expect("operational tunnel reader boundary");

    assert_eq!(
        tunnel_reader
            .matches("FROM telemetry_current_tunnels telemetry")
            .count(),
        1,
        "the canonical SQL relation must be the only plan-current owner"
    );
    assert_eq!(
        tunnel_reader
            .matches("telemetry.current_plan#>>'{runtime_control,manager}'")
            .count(),
        1,
        "the canonical serialized manager default must be normalized once"
    );
    assert!(tunnel_reader.contains("'agent_builtin'\n                        ) AS ownership_mode"));
    assert!(tunnel_reader.contains("current_plan_policy.ownership_mode"));
    assert!(tunnel_reader.contains("END AS mutation_policy"));
    assert!(tunnel_reader.contains("AS telemetry_plan_runtime_manager"));
    assert!(!tunnel_reader.contains("FROM telemetry_tunnels telemetry"));
    assert!(!tunnel_reader.contains("list_tunnel_plans"));
    assert!(!tunnel_reader.contains("list_runtime_config_apply_records"));
    assert!(!source.contains("retain_declared_telemetry_tunnels"));

    let migration = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");
    assert!(migration.contains("CREATE VIEW public.telemetry_current_tunnels"));
    assert!(migration.contains("current_plan.name = tunnel.telemetry_plan_name"));
    assert!(migration.contains("current_plan.kind = tunnel.kind"));
    assert!(migration.contains("current_plan.plan->>'interface_name' = tunnel.interface"));
    assert!(migration.contains("octet_length(tunnel.telemetry_plan_name) BETWEEN 1 AND 128"));
    assert!(migration.contains("CREATE FUNCTION public.telemetry_interface_is_admitted_resolved("));
    assert!(!migration.contains("CREATE FUNCTION public.resolve_telemetry_interface_policies("));
    let traffic_schema = include_str!("../../../../../migrations/0005_traffic_accounting.sql");
    assert!(traffic_schema.contains("CREATE FUNCTION public.resolve_telemetry_interface_policies("));
    assert!(traffic_schema.contains("FROM public.tunnel_plans plan"));
    assert!(traffic_schema.contains("plan.left_client_id AS client_id"));
    assert!(traffic_schema.contains("plan.right_client_id AS client_id"));
    assert!(!migration.contains("sync_telemetry_network_selection_after_tunnel"));
    assert!(migration.contains("CREATE TRIGGER tunnel_plans_dashboard_selection_after_insert"));
    let (_, plan_trigger) = migration
        .split_once(
            "CREATE TRIGGER tunnel_plans_dashboard_selection_after_managed_interface_update",
        )
        .expect("plan identity trigger");
    let (plan_trigger, _) = plan_trigger
        .split_once("CREATE TRIGGER tunnel_plans_dashboard_selection_after_delete")
        .expect("plan identity trigger boundary");
    assert!(plan_trigger.contains("enabled, left_client_id, right_client_id, plan, deleted_at"));
    assert!(!plan_trigger.contains("name, kind, enabled"));
    assert!(plan_trigger.contains("OLD.plan ->> 'interface_name'"));
    assert!(plan_trigger.contains("NEW.plan ->> 'interface_name'"));
    assert!(migration.contains("WHEN (NEW.enabled IS TRUE AND NEW.deleted_at IS NULL)"));
    assert!(migration.contains("WHEN (OLD.enabled IS TRUE AND OLD.deleted_at IS NULL)"));

    let tunnel_schema = include_str!("../../../../../migrations/0004_network_tunnels.sql");
    assert!(tunnel_schema.contains("CONSTRAINT tunnel_plans_name_check"));
    assert!(tunnel_schema.contains("CONSTRAINT telemetry_tunnels_plan_name_check"));
    assert!(tunnel_schema.contains("tunnel_plans_current_right_interface_idx"));
    assert!(tunnel_schema.contains("right_client_id, ((plan ->> 'interface_name'::text)), id"));
    assert!(tunnel_schema.contains("network_observation_series_active_client_idx"));
    assert!(tunnel_schema.contains("(client_id, id) WHERE (active IS TRUE)"));
}

#[test]
fn network_interface_policy_has_one_coherent_api_read_boundary() {
    let source = include_str!("repository_telemetry_rollups.rs");

    let first_page =
        raw_telemetry_network_rate_candidate_keys_sql(false, false, false, false, false, 5);
    assert!(first_page.contains("WITH candidate_keys AS MATERIALIZED"));
    assert!(first_page.contains("resolved_interface_policies AS MATERIALIZED"));
    assert!(first_page.contains("admitted_keys AS MATERIALIZED"));
    assert_eq!(
        first_page
            .matches("public.resolve_telemetry_interface_policies(")
            .count(),
        1
    );
    assert_eq!(
        first_page
            .matches("public.telemetry_interface_is_admitted_resolved(")
            .count(),
        1
    );
    assert!(!first_page.contains("public.telemetry_interface_is_admitted("));
    assert_eq!(first_page.matches("FROM admitted_keys admitted").count(), 1);
    assert!(
        first_page.find("LIMIT 5").unwrap()
            < first_page.find("admitted_keys AS MATERIALIZED").unwrap(),
        "current admission must remain after the physical candidate page"
    );
    let continuation =
        raw_telemetry_network_rate_candidate_keys_sql(false, false, false, false, true, 5);
    assert_eq!(
        continuation
            .matches("public.telemetry_interface_is_admitted_resolved(")
            .count(),
        1,
        "resolved policy must be evaluated once per distinct host stream"
    );
    assert_eq!(
        continuation
            .matches("public.resolve_telemetry_interface_policies(")
            .count(),
        1,
        "current policy relations must be resolved once for the request"
    );
    assert!(!continuation.contains("public.telemetry_interface_is_admitted("));
    assert_eq!(
        continuation.matches("FROM admitted_keys admitted").count(),
        1,
        "all mixed-order physical branches must share one bounded admission pass"
    );
    for query in [
        LATEST_TELEMETRY_NETWORK_RATES_SQL,
        TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL,
    ] {
        assert!(query.contains("public.resolve_telemetry_interface_policies("));
        assert!(query.contains("public.telemetry_interface_is_admitted_resolved("));
        assert!(!query.contains("public.telemetry_interface_is_admitted("));
    }

    let (_, raw_dashboard) = source
        .split_once("async fn list_dashboard_raw_telemetry_network_rates_selected_with_output")
        .expect("raw dashboard network reader");
    let (raw_dashboard, _) = raw_dashboard
        .split_once("pub(crate) async fn dashboard_telemetry_start_unix")
        .expect("raw dashboard network reader boundary");
    assert!(raw_dashboard.contains("query_projected_telemetry_network_history("));
    assert!(raw_dashboard.contains("project_network_rate_selection("));
    assert!(!raw_dashboard.contains("telemetry_samples"));
    assert!(!raw_dashboard.contains("resolve_telemetry_interface_policies"));

    let (_, samples) = source
        .split_once("pub(crate) async fn list_telemetry_samples")
        .expect("raw telemetry sample reader");
    let (samples, _) = samples
        .split_once("pub(crate) async fn list_dashboard_raw_telemetry_rollups")
        .expect("raw telemetry sample reader boundary");
    assert!(samples.contains("interface_candidates AS MATERIALIZED"));
    assert!(samples.contains("admitted_interfaces AS MATERIALIZED"));
    assert_eq!(
        samples
            .matches("public.telemetry_interface_is_admitted_resolved(")
            .count(),
        1,
        "resolved policy must be evaluated once per distinct page interface"
    );
    assert_eq!(
        samples
            .matches("public.resolve_telemetry_interface_policies(")
            .count(),
        1,
        "policy relations must be read once for the page"
    );
    assert!(!samples.contains("public.telemetry_interface_is_admitted("));
    assert!(samples.contains("admitted.source_kind = 'host'"));
    assert!(samples.contains("admitted.source_kind = 'tunnel'"));

    let (_, current_query) = source
        .split_once("WITH online_current_interfaces AS MATERIALIZED")
        .expect("current network reader");
    let (current_query, _) = current_query
        .split_once("// Explicit physical-tier inspection")
        .expect("current network reader boundary");
    assert!(current_query.contains("public.resolve_telemetry_interface_policies("));
    assert!(current_query.contains("public.telemetry_interface_is_admitted_resolved("));
    assert!(!current_query.contains("public.telemetry_interface_is_admitted("));
    assert!(!current_query.contains("$9::BOOLEAN"));
    assert!(!current_query.contains("INTERVAL '15 minutes'"));
    assert!(!current_query.contains("LatestNetworkRateVisibility::SingleVpsDetail"));
    assert!(source.contains("RECENT_EXCLUDED_NETWORK_TRANSITIONS_SQL"));

    let (_, generic_latest) = source
        .split_once("pub(crate) async fn list_latest_telemetry_network_rates(")
        .expect("generic latest reader");
    let (generic_latest, snapshot_latest) = generic_latest
        .split_once("pub(crate) async fn list_latest_telemetry_network_rates_for_clients(")
        .expect("snapshot latest reader");
    assert!(generic_latest.contains("LatestNetworkRateVisibility::AdmittedOnly"));
    let (snapshot_latest, selected_latest) = snapshot_latest
        .split_once("pub(crate) async fn list_latest_telemetry_network_rates_for_vps_detail(")
        .expect("single-VPS detail reader");
    assert!(snapshot_latest.contains("LatestNetworkRateVisibility::AdmittedOnly"));
    let (detail_latest, selected_latest) = selected_latest
        .split_once("pub(crate) async fn list_latest_telemetry_network_rates_for_selection(")
        .expect("selected latest reader");
    assert!(detail_latest.contains("LatestNetworkRateVisibility::SingleVpsDetail"));
    let (selected_latest, _) = selected_latest
        .split_once("async fn list_latest_telemetry_network_rates_matching(")
        .expect("latest reader implementation");
    assert!(selected_latest.contains("LatestNetworkRateVisibility::AdmittedOnly"));

    let history = include_str!("../system/repository_history.rs");
    let (_, traffic_exports) = history
        .split_once("pub(crate) async fn export_traffic_counter_samples")
        .expect("traffic export boundary");
    let (traffic_exports, _) = traffic_exports
        .split_once("pub(crate) async fn export_job_outputs")
        .expect("traffic export boundary end");
    assert_eq!(
        traffic_exports
            .matches("public.telemetry_interface_is_admitted_resolved(")
            .count(),
        2,
        "exact and rolled-up generic traffic exports share admission"
    );
    assert_eq!(
        traffic_exports
            .matches("public.resolve_telemetry_interface_policies(")
            .count(),
        2,
        "each export resolves all requested clients setwise"
    );
    assert!(!traffic_exports.contains("public.telemetry_interface_is_admitted("));

    let (_, tunnel_reader) = source
        .split_once("async fn list_telemetry_tunnels_matching(")
        .expect("tunnel lifecycle reader");
    assert_eq!(
        tunnel_reader
            .matches("public.telemetry_interface_is_admitted_resolved(")
            .count(),
        1,
        "tunnel byte admission uses the pure predicate per operational row"
    );
    assert_eq!(
        tunnel_reader
            .matches("public.resolve_telemetry_interface_policies(")
            .count(),
        1,
        "tunnel policies must be relation-resolved once for the request"
    );
    assert!(!tunnel_reader.contains("public.telemetry_interface_is_admitted("));
    assert!(tunnel_reader.contains("telemetry.counters_admitted_at_projection"));
    assert!(tunnel_reader.contains("interface_policy.admitted"));
    assert!(tunnel_reader.contains("THEN telemetry.rx_bytes"));
    assert!(tunnel_reader.contains("THEN telemetry.tx_bytes"));
    assert!(tunnel_reader.contains("TunnelCounterVisibility::SingleVpsDetail"));
    assert!(tunnel_reader.contains("INTERVAL '15 minutes'"));
    assert!(
        !tunnel_reader.contains("WHERE interface_policy.admitted"),
        "network byte admission must not own tunnel operational lifecycle"
    );
}

#[test]
fn explicit_tier_latest_network_reader_uses_one_bounded_edge_owner() {
    let sql = LATEST_TELEMETRY_NETWORK_RATES_SQL;

    assert!(sql.contains("durable_candidate_keys AS MATERIALIZED"));
    assert!(sql.contains("candidate_projected_suffix AS MATERIALIZED"));
    assert!(sql.contains("candidate_stream_keys AS MATERIALIZED"));
    assert!(sql.contains("FROM candidate_projected_suffix suffix\n    ) candidate"));
    assert!(sql.contains("projected_suffix AS MATERIALIZED"));
    assert_eq!(
        sql.matches("FROM telemetry_projected_raw_network_minutes_source(ARRAY(")
            .count(),
        1,
        "the projected raw suffix must be resolved once per request"
    );
    assert_eq!(
        sql.matches("FROM telemetry_network_rates retained").count(),
        1
    );
    assert_eq!(
        sql.matches("FROM traffic_counter_samples sample").count(),
        1
    );
    assert_eq!(sql.matches("FROM projected_suffix suffix").count(), 1);
    assert!(sql.contains("stream.first_unpromoted_observed_at IS NOT NULL"));
    assert!(sql.contains(
        "sample.observed_at >=\n                      stream.first_unpromoted_observed_at"
    ));
    assert!(sql.contains("DISTINCT ON (candidate.latest_observed_at)"));
    assert!(sql.contains("previous.recency_rank = 2"));
    assert!(sql.contains("latest.recency_rank = 1"));
    assert!(sql.contains("retained.bucket_secs = $4"));
    assert!(sql.contains("WHERE $4::INTEGER = 60"));
}

#[test]
fn dashboard_exact_sources_choose_physical_owners_before_history_reads() {
    let telemetry = include_str!("../../../../../migrations/0003_telemetry_core.sql");
    let (_, network) = telemetry
        .split_once("CREATE FUNCTION public.telemetry_network_durable_points_source(")
        .expect("durable network source");
    let (network, _) = network
        .split_once("CREATE FUNCTION public.telemetry_network_rate_points_source(")
        .expect("durable network source boundary");
    assert!(network.contains("LANGUAGE plpgsql"));
    assert!(network.contains("IF p_bucket_secs IS NULL OR p_bucket_secs = 60 THEN"));
    assert!(network.contains("FROM public.telemetry_network_rates_minute retained"));
    assert_eq!(network.matches("IF p_bucket_secs IS NULL THEN").count(), 3);
    assert_eq!(network.matches("ELSIF p_bucket_secs <> 60 THEN").count(), 3);
    assert!(network.contains("FROM public.telemetry_network_rates_coarse retained"));
    assert_eq!(
        network
            .matches("FROM public.telemetry_network_rates_coarse retained")
            .count(),
        8
    );
    assert_eq!(
        network
            .matches("retained.bucket_secs = p_bucket_secs")
            .count(),
        4
    );
    assert!(!network.contains("p_bucket_secs IS NULL\n                  OR retained.bucket_secs"));
    assert!(network.contains("retained.client_id = ANY(p_client_ids)"));
    assert!(network.contains("sample.client_id = ANY(p_client_ids)"));
    assert!(network.contains("p_interfaces TEXT[] DEFAULT NULL"));
    assert!(network.contains("IF p_client_ids IS NULL AND p_interfaces IS NOT NULL THEN"));
    assert_eq!(
        network
            .matches("retained.interface = ANY(p_interfaces)")
            .count(),
        7
    );
    assert_eq!(
        network
            .matches("sample.interface = ANY(p_interfaces)")
            .count(),
        3
    );
    assert!(!network.contains("FROM public.telemetry_network_rates retained"));

    let dashboard = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");
    let (_, traffic) = dashboard
        .split_once("CREATE FUNCTION public.telemetry_dashboard_traffic_source_points(")
        .expect("dashboard traffic source");
    let (traffic, _) = traffic
        .split_once("CREATE FUNCTION public.telemetry_dashboard_traffic_overlay_source(")
        .expect("dashboard traffic source boundary");
    assert!(traffic.contains("LANGUAGE plpgsql"));
    assert!(traffic.contains("IF p_source_bucket_secs IS NULL OR p_source_bucket_secs = 60 THEN"));
    assert!(traffic.contains("IF p_source_bucket_secs IS NULL OR p_source_bucket_secs <> 60 THEN"));
    assert!(traffic.contains("FROM raw source\n            WHERE source.usage_authoritative"));
    assert!(traffic.contains("WHERE NOT source.usage_authoritative"));
    assert_eq!(
        traffic
            .matches("JOIN public.traffic_counter_rollups rollup")
            .count(),
        1
    );
}

#[test]
fn projected_raw_sources_bind_exact_consumers_before_expansion() {
    let schema = include_str!("../../../../../migrations/0003_telemetry_core.sql");
    let (_, resource) = schema
        .split_once("CREATE FUNCTION public.telemetry_projected_raw_resource_minutes_source(")
        .expect("projected raw resource source");
    let (resource, _) = resource
        .split_once("CREATE FUNCTION public.telemetry_resource_points_source(")
        .expect("projected raw resource source boundary");
    assert!(resource.contains("requested_clients AS MATERIALIZED"));
    assert!(resource.contains("requested_heads AS MATERIALIZED"));
    assert!(resource.contains("FROM requested_heads head"));
    assert!(resource.contains("sample.accepted_seq > head.materialized_seq"));
    assert!(resource.contains("sample.accepted_seq <= head.projected_seq"));

    let (_, ping) = schema
        .split_once("CREATE VIEW public.telemetry_projected_raw_ping_minutes AS")
        .expect("projected raw ping view");
    let (ping, _) = ping
        .split_once("CREATE VIEW public.telemetry_ping_points AS")
        .expect("projected raw ping boundary");
    for stage in [
        "expanded",
        "raw_evidence",
        "touched",
        "evidence",
        "canonical",
        "grouped",
    ] {
        assert!(ping.contains(&format!("{stage} AS NOT MATERIALIZED")));
    }

    let (_, network) = schema
        .split_once("CREATE FUNCTION public.telemetry_projected_raw_network_minutes_source(")
        .expect("projected raw network source");
    let (network, _) = network
        .split_once("CREATE FUNCTION public.telemetry_network_durable_points_source(")
        .expect("projected raw network source boundary");
    assert!(network.contains("requested_clients AS MATERIALIZED"));
    assert!(network.contains("requested_heads AS MATERIALIZED"));
    assert!(network.contains("raw_client_minutes AS MATERIALIZED"));
    assert!(network.contains("expanded AS MATERIALIZED"));
    assert_eq!(network.matches("jsonb_array_elements(").count(), 1);
    assert!(!network.contains("telemetry_projected_raw_host_network_observations"));
    assert!(network.contains("JOIN expanded"));
    assert!(network.contains("touched_predecessors AS MATERIALIZED"));
    assert!(network.contains("durable_predecessor_bucket_start"));
    assert!(!network.contains("open_segment"));
    assert!(!network.contains("extract(epoch FROM bucket_start)::BIGINT / 60"));
    assert!(network.contains("LEFT JOIN public.traffic_counter_streams stream"));
    assert!(network.contains(
        "AND shadow.bucket_start = date_trunc(\n                        'minute', stream.latest_sample_observed_at"
    ));
    assert!(network.contains(") predecessor ON TRUE"));
    assert_eq!(
        network
            .matches(
                "PARTITION BY source.client_id, source.interface,\n                     source.durable_predecessor_bucket_start"
            )
            .count(),
        1
    );
    assert_eq!(
        network
            .matches(
                "PARTITION BY client_id, interface,\n                     durable_predecessor_bucket_start"
            )
            .count(),
        1
    );
    assert!(
        !network.contains("PARTITION BY source.client_id, source.interface, source.bucket_start")
    );

    let dashboard = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");
    let (_, resource_source) = dashboard
        .split_once("CREATE FUNCTION public.telemetry_dashboard_resource_overlay_source(")
        .expect("resource overlay source");
    let (resource_source, _) = resource_source
        .split_once("CREATE FUNCTION public.telemetry_dashboard_network_overlay_source(")
        .expect("resource overlay source boundary");
    assert!(
        resource_source.contains("FROM public.telemetry_projected_raw_resource_minutes_source(")
    );
    assert!(resource_source.contains("requested_blocks AS MATERIALIZED"));
    assert!(!resource_source.contains("telemetry_rollups"));

    let (_, network_source) = dashboard
        .split_once("CREATE FUNCTION public.telemetry_dashboard_network_overlay_source(")
        .expect("network overlay source");
    let (network_source, _) = network_source
        .split_once("CREATE INDEX telemetry_dashboard_block_events_client_age_idx")
        .expect("network overlay source boundary");
    assert!(network_source.contains("FROM public.telemetry_projected_raw_network_minutes_source("));
    assert!(network_source.contains("requested_blocks AS MATERIALIZED"));
    assert!(
        dashboard
            .matches("public.telemetry_network_durable_points_source(")
            .count()
            >= 3
    );

    let (_, traffic_source) = dashboard
        .split_once("CREATE FUNCTION public.telemetry_dashboard_traffic_overlay_source(")
        .expect("traffic overlay source");
    let (traffic_source, _) = traffic_source
        .split_once("CREATE FUNCTION public.telemetry_dashboard_resource_overlay_source(")
        .expect("traffic overlay source boundary");
    assert!(traffic_source.contains("requested_heads AS MATERIALIZED"));
    assert!(traffic_source.contains("head.client_id = ANY(p_client_ids)"));
    assert!(traffic_source.contains("JOIN public.traffic_counter_minute_heads minute"));
    assert!(traffic_source.contains("JOIN public.telemetry_samples sample"));
    assert!(traffic_source
        .contains("CROSS JOIN LATERAL public.telemetry_dashboard_traffic_source_points("));
    assert!(traffic_source.contains("        60,"));
}

#[test]
fn current_network_reads_choose_the_rule_matching_owner_and_tunnels_keep_projection_stamps() {
    let source = include_str!("repository_telemetry_rollups.rs");
    let (_, current_reader) = source
        .split_once("WITH online_current_interfaces AS MATERIALIZED")
        .expect("current network reader");
    let (current_reader, _) = current_reader
        .split_once("} else {")
        .expect("current network reader boundary");
    assert!(current_reader.contains("network_current.transition_admitted_at_projection"));
    assert!(current_reader.contains("public.resolve_telemetry_interface_policies("));
    assert!(current_reader.contains("public.telemetry_interface_is_admitted_resolved("));
    assert!(!current_reader.contains("public.telemetry_interface_is_admitted("));
    assert!(!current_reader.contains("AND NOT network_current.transition_admitted_at_projection"));
    assert!(!current_reader.contains("AND NOT interface_policy.admitted"));
    assert!(!current_reader.contains("$9::BOOLEAN"));
    assert!(!current_reader.contains("INTERVAL '15 minutes'"));
    assert!(source.contains("RECENT_EXCLUDED_NETWORK_TRANSITIONS_SQL"));
    assert!(source.contains("sample.observed_at >= statement_timestamp() - interval '15 minutes'"));
    assert!(source.contains("WHERE client.id = $1"));
    let (_, detail_reader) = source
        .split_once("const RECENT_EXCLUDED_NETWORK_TRANSITIONS_SQL")
        .expect("recent excluded-interface detail reader");
    let (detail_reader, _) = detail_reader
        .split_once("pub(crate) struct DashboardTelemetryNetworkProjection")
        .expect("recent excluded-interface detail reader boundary");
    assert!(!detail_reader.contains("admitted_at_projection"));
    assert!(source.contains("visibility == LatestNetworkRateVisibility::SingleVpsDetail"));
    assert!(source.contains("sqlx::query(RECENT_EXCLUDED_NETWORK_TRANSITIONS_SQL)"));

    let ingest = include_str!("repository_ingest.rs");
    assert!(!ingest.contains("telemetry_network_detail_current"));
    assert!(!ingest.contains("replace_postgres_telemetry_network_detail_current"));
    let (_, tunnel_writer) = ingest
        .split_once("async fn upsert_postgres_telemetry_tunnels")
        .expect("tunnel writer");
    assert!(tunnel_writer.contains("admission.tunnel_admitted(ordinal)"));
    assert!(tunnel_writer.contains(
        "counters_admitted_at_projection =\n                    EXCLUDED.counters_admitted_at_projection"
    ));

    let network_schema = include_str!("../../../../../migrations/0003_telemetry_core.sql");
    assert!(!network_schema.contains("telemetry_network_detail_current"));
    let (_, current_source) = network_schema
        .split_once("CREATE FUNCTION public.telemetry_network_current_source(")
        .expect("current network source");
    let (current_source, _) = current_source
        .split_once("-- Indexes.")
        .expect("current network source boundary");
    assert!(current_source.contains("stream_registry AS MATERIALIZED"));
    assert!(current_source.contains("network_payloads AS MATERIALIZED"));
    assert!(current_source.contains("valid_network_payloads AS MATERIALIZED"));
    assert!(current_source.contains("raw_streams AS MATERIALIZED"));
    assert!(current_source.contains("stream.sample_edge_revision = stream.source_revision"));
    assert!(current_source.contains("registry.sample_edge_revision = registry.source_revision"));
    assert!(current_source.contains("raw.first_bucket_start >"));
    assert!(current_source.contains("registry.latest_sample_observed_at"));
    assert!(current_source.contains("shape.anchor_rx_counter_epoch"));
    assert!(current_source.contains("base.latest_sample_effective_observed_at"));
    assert!(current_source.contains("base.previous_sample_effective_observed_at"));
    assert!(current_source.contains("base.latest_sample_count AS sample_count"));
    assert!(!current_source.contains("traffic_counter_samples"));
    assert!(!current_source.contains("touched_predecessors"));
    assert!(!current_source.contains("raw_predecessors"));
    assert!(network_schema.contains("TRUE AS admitted_at_projection"));
    assert!(current_source.contains("TRUE AS admitted_at_projection"));
    assert!(current_source.contains("public.telemetry_ordinal_admission_mask_is_exact("));
    assert!(network_schema.contains("AS latest_admitted_at_projection"));
    let tunnel_schema = include_str!("../../../../../migrations/0004_network_tunnels.sql");
    assert!(
        tunnel_schema.contains("counters_admitted_at_projection boolean DEFAULT false NOT NULL")
    );
}

#[test]
fn declared_tunnel_visibility_reads_each_client_status_once() {
    let source = include_str!("repository_telemetry_rollups.rs");
    let (_, query) = source
        .split_once("WITH visible_status AS MATERIALIZED")
        .expect("declared tunnel visibility owner");
    let (query, _) = query
        .split_once("let mut records = rows")
        .expect("declared tunnel query boundary");

    assert_eq!(query.matches("FROM visible_clients").count(), 1);
    assert!(query.contains("JOIN visible_status visible_client"));
    assert!(query.contains("visible_client.status <> 'suspended'"));
    assert!(query.contains("LEFT JOIN visible_status visible_peer"));
    assert!(query.contains("visible_peer.status IS DISTINCT FROM 'suspended'"));
    assert!(!query.contains("suspended_peer"));
    assert!(!query.contains("suspended_endpoint"));
}

#[test]
fn raw_coverage_uses_compact_resource_network_and_ping_heads() {
    let sql = RAW_TELEMETRY_COVERS_RANGE_START_SQL;

    assert!(sql.contains("telemetry_dashboard_projection_heads"));
    for boundary in ["resource_first_at", "network_first_at", "ping_first_at"] {
        assert!(sql.contains(boundary), "missing {boundary}");
    }
    for retained_fact in [
        "FROM telemetry_rollups",
        "FROM telemetry_network_rates",
        "FROM telemetry_ping_rollups",
        "FROM traffic_counter_samples",
    ] {
        assert!(
            !sql.contains(retained_fact),
            "range preflight must not scan {retained_fact}"
        );
    }
    assert!(sql.contains("ORDER BY sample.observed_at ASC"));
    assert!(sql.contains("LIMIT 1"));
}

#[test]
fn client_detail_projection_preserves_complete_resource_and_interface_contracts() {
    let resource = TELEMETRY_RESOURCE_HISTORY_PROJECTION_SQL;
    assert!(resource.contains("JOIN telemetry_resource_points_source("));
    assert!(resource.contains("$1::TEXT[],"));
    assert!(resource.contains("telemetry_dashboard_resource_projection_heads"));
    assert!(resource
        .contains("GROUP BY source.client_id, source.chart_start_unix, source.effective_step"));
    assert!(resource.contains("GREATEST($4, rollup.bucket_secs)"));
    assert!(resource.contains("rollup.bucket_start <= to_timestamp($3)"));
    assert!(resource.contains("> to_timestamp($2)"));
    assert!(resource.contains("ranked.recency_rank <= $5"));
    assert!(resource.contains("$5::BIGINT * (($4::BIGINT + 59) / 60)"));
    assert!(!resource.contains("NULL::INTEGER,\n        NULL::BIGINT"));
    let largest_normalized_step = normalized_dashboard_step_secs(i32::MAX);
    let rows_per_point = (i64::from(largest_normalized_step) + 59) / 60;
    assert!(1_440_i64.checked_mul(rows_per_point).is_some());
    let schema = include_str!("../../../../../migrations/0003_telemetry_core.sql");
    let (_, resource_source) = schema
        .split_once("CREATE FUNCTION public.telemetry_resource_points_source(")
        .expect("canonical resource source");
    let (resource_source, _) = resource_source
        .split_once("CREATE VIEW public.telemetry_projected_raw_ping_minutes AS")
        .expect("canonical resource source boundary");
    let (_, exact_source) = resource_source
        .split_once("exact_client_points AS NOT MATERIALIZED")
        .expect("exact resource owner");
    let shadow = exact_source
        .find("AND NOT EXISTS (")
        .expect("projected suffix shadow replacement");
    let retained_limit = exact_source
        .find("LIMIT p_per_owner_limit")
        .expect("retained physical cap");
    let suffix_merge = exact_source
        .find("SELECT suffix.*\n            FROM projected_suffix suffix")
        .expect("projected suffix merge");
    let merged_limit = exact_source
        .rfind("LIMIT p_per_owner_limit")
        .expect("merged physical cap");
    assert!(
        shadow < retained_limit && retained_limit < suffix_merge && suffix_merge < merged_limit
    );
    for field in [
        "cpu_usage_sample_count",
        "cpu_load_15_sum",
        "memory_available_bytes_sum",
        "swap_available_bytes_sum",
        "disk_available_bytes_sum",
        "connections_observed_at",
        "latest_observed_at",
        "updated_at",
    ] {
        assert!(resource.contains(field), "resource detail omits {field}");
    }
    assert!(resource.contains("recent.connections_observed_at::TEXT"));
    assert!(resource.contains("recent.latest_observed_at::TEXT"));
    assert!(!resource.contains("connections_observed_unix"));
    assert!(!resource.contains("latest_observed_unix"));
    assert!(!resource.contains("telemetry_dashboard_resource_sparse_states"));
    assert!(!resource.contains("telemetry_resource_summary_merge_agg"));

    let network = TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL;
    assert!(network.contains("telemetry_dashboard_network_projection_heads"));
    assert!(network.contains("unnest(heads.network_generation_interfaces)"));
    assert!(network.contains("FROM telemetry_projected_raw_network_minutes_source($1::TEXT[])"));
    assert!(network.contains("CROSS JOIN LATERAL telemetry_network_durable_points_source("));
    assert!(network.contains("selected_stream_arrays AS MATERIALIZED"));
    assert!(network.contains("streams.client_ids,"));
    assert!(network.contains("streams.interfaces,"));
    assert!(network.contains(
        "GROUP BY source.client_id, source.interface,\n             source.chart_start_unix, source.effective_step"
    ));
    assert!(network.contains("'predecessor'::TEXT"));
    assert!(network.contains("PARTITION BY state.client_id, state.interface"));
    assert!(network.contains("state.rx_counter_epoch = state.previous_rx_epoch"));
    assert!(network.contains("derived.sample_count, derived.rx_bytes_avg"));
    assert!(network.contains("derived.rx_bytes_avg, derived.tx_bytes_avg"));
    assert!(!network.contains("sum(point.rx_bytes_avg"));
    assert!(network.contains("output.updated_at"));
    assert!(!network.contains("telemetry_dashboard_network_sparse_states"));
    assert!(!network.contains("telemetry_network_summary_expand("));
    assert!(!network.contains("top_clients"));
    assert!(!network.contains("fleet_points"));
}

#[test]
fn unified_detail_network_caps_native_bins_before_prefix_and_reset_without_refill() {
    let sql = TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL;
    let native_bins = sql.find("bucket_values AS").expect("native bins");
    let cap = sql
        .find("capped_points AS MATERIALIZED")
        .expect("newest candidate cap");
    let prefix = sql
        .find("interface_states AS MATERIALIZED")
        .expect("prefix predecessor assembly");
    let reset = sql.find("derived AS MATERIALIZED").expect("reset filter");
    let output = sql.find("output AS MATERIALIZED").expect("detail output");

    assert!(native_bins < cap && cap < prefix && prefix < reset && reset < output);
    assert!(sql.contains("point.recency_rank <= $5"));
    assert!(sql.contains("FROM capped_points point\n    UNION ALL"));
    assert!(sql.contains("point.recency_rank = $5 + 1"));
    for physical_owner in ["retained", "sample", "suffix"] {
        assert!(sql.contains(&format!(
            "{physical_owner}.latest_observed_at < oldest.first_source_start"
        )));
    }
    assert!(sql.contains("candidate.bucket_secs DESC,\n                 candidate.source_priority ASC\n        LIMIT 1"));
    assert!(!sql[reset..output].contains("row_number()"));
    assert!(!sql[reset..output].contains("LIMIT"));
}

#[test]
fn unified_projection_preserves_fractional_resource_times_nulls_and_terminal_network_averages() {
    let resource = TELEMETRY_RESOURCE_HISTORY_PROJECTION_SQL;
    assert!(resource.contains("recent.connections_observed_at::TEXT"));
    assert!(resource.contains("recent.latest_observed_at::TEXT"));
    assert!(!resource.contains("date_trunc('second'"));
    assert!(!resource.contains("COALESCE(recent.connections_observed_at"));

    let network = TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL;
    assert!(network.contains("extract(epoch FROM rate.latest_observed_at)::BIGINT"));
    assert!(network.contains("max(source.terminal_values) AS terminal_values"));
    assert!(network.contains("to_timestamp(row.terminal_values[1])"));
    assert!(network.contains("derived.sample_count, derived.rx_bytes_avg, derived.tx_bytes_avg"));
    for weighted_reduction in [
        "sum(point.rx_bytes_avg",
        "sum(point.tx_bytes_avg",
        "point.rx_bytes_avg * point.sample_count",
        "point.tx_bytes_avg * point.sample_count",
    ] {
        assert!(!network.contains(weighted_reduction));
    }
}

#[test]
fn client_detail_queries_keep_one_logical_point_owner() {
    for (sql, fact, head) in [
        (
            TELEMETRY_RESOURCE_HISTORY_PROJECTION_SQL,
            "JOIN telemetry_resource_points_source(",
            "telemetry_dashboard_resource_projection_heads",
        ),
        (
            TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL,
            "CROSS JOIN LATERAL telemetry_network_durable_points_source(",
            "telemetry_dashboard_network_projection_heads",
        ),
    ] {
        assert_eq!(sql.matches(fact).count(), 1, "detail fact has two owners");
        assert!(sql.contains(head), "detail readiness head missing");
        for forbidden in [
            "telemetry_dashboard_resource_sparse_states(",
            "telemetry_dashboard_network_sparse_states(",
            "telemetry_resource_summary_nodes",
            "telemetry_network_summary_nodes",
            "range_agg(",
            "generate_series(",
        ] {
            assert!(!sql.contains(forbidden), "detail read contains {forbidden}");
        }
    }

    let repository = include_str!("repository_telemetry_rollups.rs");
    let (_, latest) = repository
        .split_once("async fn list_latest_telemetry_rollups_matching")
        .expect("latest resource reader");
    let (latest, _) = latest
        .split_once("pub(crate) async fn list_projected_telemetry_network_history")
        .expect("latest resource reader end");
    assert!(latest.contains("FROM visible_clients visible"));
    assert!(latest.contains("CROSS JOIN LATERAL"));
    assert!(latest.contains("projected_suffix AS MATERIALIZED"));
    assert_eq!(
        latest
            .matches("FROM telemetry_projected_raw_resource_minutes_source(ARRAY(")
            .count(),
        1,
        "the projected resource suffix must be resolved once per request"
    );
    assert_eq!(latest.matches("FROM telemetry_rollups retained").count(), 1);
    assert_eq!(latest.matches("FROM projected_suffix suffix").count(), 1);
    assert!(latest.contains("FROM projected_suffix shadow"));
    assert!(!latest.contains("telemetry_resource_active"));
    assert!(!latest.contains("DISTINCT ON"));

    let telemetry_schema = include_str!("../../../../../migrations/0003_telemetry_core.sql");
    assert!(telemetry_schema.contains(
        "CREATE INDEX telemetry_rollups_client_latest_point_idx ON public.telemetry_rollups USING btree (client_id, bucket_start DESC, latest_observed_at DESC, bucket_secs ASC);"
    ));
}

#[test]
fn client_detail_repository_has_no_second_owner_fallback() {
    let source = include_str!("repository_telemetry_rollups.rs");
    let (_, resource_owner) = source
        .split_once("async fn query_telemetry_resource_history")
        .expect("shared resource history owner");
    let (resource_owner, _) = resource_owner
        .split_once("pub(crate) async fn list_dashboard_raw_telemetry_network_rates")
        .expect("shared resource history owner end");
    let (_, resource) = source
        .split_once("pub(crate) async fn list_projected_telemetry_resource_history")
        .expect("resource detail projection");
    let (resource, remainder) = resource
        .split_once("pub(crate) async fn list_telemetry_rollups")
        .expect("resource detail projection end");
    let (_, network) = remainder
        .split_once("pub(crate) async fn list_projected_telemetry_network_history")
        .expect("network detail projection");
    let (network, _) = network
        .split_once("pub(crate) async fn list_telemetry_network_rates")
        .expect("network detail projection end");

    assert!(resource_owner.contains("TELEMETRY_RESOURCE_HISTORY_PROJECTION_SQL"));
    assert!(resource.contains("query_telemetry_resource_history("));
    assert!(resource.contains("true,"));
    assert!(network.contains("TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL"));
    for projection in [resource_owner, resource, network] {
        assert!(!projection.contains("fallback"));
        assert!(!projection.contains("list_dashboard_telemetry_rollups"));
        assert!(!projection.contains("list_dashboard_telemetry_network_rates_selected"));
    }
}

#[test]
fn selected_network_history_aggregation_preserves_the_existing_card_contract() {
    let selected_streams = TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL
        .find("selected_streams AS MATERIALIZED")
        .expect("selected stream boundary");
    let transition_derivation = TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL
        .find("derived AS MATERIALIZED")
        .expect("per-interface transition boundary");
    let aggregate_output = TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL
        .find("selected_output AS MATERIALIZED")
        .expect("selected aggregate boundary");
    assert!(selected_streams < transition_derivation);
    assert!(transition_derivation < aggregate_output);
    assert!(TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL
        .contains("FROM UNNEST($7::TEXT[], $8::TEXT[])\n        selected(client_id, interface)"));
    assert!(TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL
        .contains("PARTITION BY state.client_id, state.interface"));
    assert!(TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL.contains(
        "GROUP BY derived.client_id, derived.chart_start_unix,\n             derived.effective_step"
    ));

    let point = |interface: &str,
                 sample_count: i32,
                 rx_bytes_avg: i64,
                 tx_bytes_avg: i64,
                 rx_bytes_delta: i64,
                 tx_bytes_delta: i64,
                 rx_bps_avg: f64,
                 tx_bps_avg: f64,
                 latest_observed_at: &str,
                 updated_at: &str| TelemetryNetworkRateView {
        client_id: "vps-1".to_string(),
        interface: interface.to_string(),
        bucket_start: "2026-08-30T10:00:00+00:00".to_string(),
        bucket_secs: 60,
        sample_count,
        rx_bytes_avg,
        tx_bytes_avg,
        latest_observed_at: latest_observed_at.to_string(),
        rx_bytes_delta,
        tx_bytes_delta,
        rx_bps_avg,
        tx_bps_avg,
        updated_at: updated_at.to_string(),
    };
    let rows = aggregate_selected_network_history_oracle(vec![
        point("eth0", 2, 10, 20, 3, 4, 5.0, 6.0, "10:00:30", "10:00:31"),
        point("eth1", 3, 30, 40, 7, 8, 9.0, 10.0, "10:00:40", "10:00:41"),
    ]);

    assert_eq!(rows.len(), 1);
    let aggregate = &rows[0];
    assert!(aggregate.interface.is_empty());
    assert_eq!(aggregate.sample_count, 3);
    assert_eq!(aggregate.rx_bytes_avg, 40);
    assert_eq!(aggregate.tx_bytes_avg, 60);
    assert_eq!(aggregate.rx_bytes_delta, 10);
    assert_eq!(aggregate.tx_bytes_delta, 12);
    assert_eq!(aggregate.rx_bps_avg, 14.0);
    assert_eq!(aggregate.tx_bps_avg, 16.0);
    assert_eq!(aggregate.latest_observed_at, "10:00:40");
    assert_eq!(aggregate.updated_at, "10:00:41");
}

#[test]
fn raw_network_list_is_candidate_driven_and_refills_only_after_exact_validation() {
    let first_page =
        raw_telemetry_network_rate_candidate_keys_sql(true, true, true, true, false, 5);
    let continuation =
        raw_telemetry_network_rate_candidate_keys_sql(false, false, false, true, true, 5);
    let payload = raw_telemetry_network_rate_payload_sql(true);
    let cross_tier_payload = raw_telemetry_network_rate_payload_sql(false);

    assert!(!first_page.contains("lag("));
    assert!(!first_page.contains("WINDOW rate_window"));
    assert!(!first_page.contains("IS NULL OR"));
    assert!(!first_page.contains("FROM clients"));
    assert!(!first_page.contains("visible_clients"));
    assert!(first_page.contains("WITH candidate_keys AS MATERIALIZED"));
    assert!(first_page.contains("admitted_keys AS MATERIALIZED"));
    assert!(first_page.contains("FROM telemetry_network_rates_minute rate"));
    assert!(first_page.contains("FROM telemetry_network_rates_coarse rate"));
    assert!(first_page.contains("FROM traffic_counter_samples sample"));
    assert!(first_page.contains("FROM telemetry_projected_raw_network_minutes_source("));
    assert!(first_page.contains("ARRAY[$1::TEXT]"));
    assert_eq!(
        first_page
            .matches("public.telemetry_interface_is_admitted_resolved(")
            .count(),
        1
    );
    assert_eq!(
        first_page
            .matches("public.resolve_telemetry_interface_policies(")
            .count(),
        1
    );
    assert!(!first_page.contains("public.telemetry_interface_is_admitted("));
    assert_eq!(first_page.matches("FROM admitted_keys admitted").count(), 1);
    assert!(!first_page.contains("LEFT JOIN LATERAL"));
    assert!(!first_page.contains("transition_valid"));
    assert!(first_page.contains("sample.source_kind = 'host'"));
    assert!(first_page.contains("candidate.client_id = $1"));
    assert!(first_page.contains("candidate.interface = $2"));
    assert!(first_page.contains("candidate.bucket_secs = $3"));
    assert!(first_page.contains("candidate.client_id = ANY($4::TEXT[])"));
    assert!(first_page.contains(
        "candidate.interface ASC,\n         candidate.bucket_start DESC,\n         candidate.bucket_secs DESC"
    ));
    assert_eq!(first_page.matches("LIMIT 5").count(), 4);
    assert!(first_page.contains(") AS admitted"));

    for strict_suffix in [
        "candidate.latest_observed_at < $2::TIMESTAMPTZ",
        "candidate.client_id > $3::TEXT",
        "candidate.interface > $4::TEXT",
        "candidate.bucket_start < $5::TIMESTAMPTZ",
        "candidate.bucket_secs < $6::INTEGER",
    ] {
        assert!(
            continuation.contains(strict_suffix),
            "continuation lost mixed-order suffix: {strict_suffix}"
        );
    }
    assert!(continuation.contains("UNION ALL"));
    assert_eq!(continuation.matches("LIMIT 5").count(), 21);
    assert_eq!(
        continuation
            .matches("public.telemetry_interface_is_admitted_resolved(")
            .count(),
        1
    );
    assert_eq!(
        continuation
            .matches("public.resolve_telemetry_interface_policies(")
            .count(),
        1
    );
    assert!(!continuation.contains("public.telemetry_interface_is_admitted("));
    assert_eq!(
        continuation.matches("FROM admitted_keys admitted").count(),
        1
    );
    assert!(!continuation.contains("OFFSET"));

    assert!(payload.contains("FROM UNNEST("));
    assert!(payload.contains(") WITH ORDINALITY AS candidate("));
    assert!(payload.contains("candidate_streams AS MATERIALIZED"));
    assert!(payload.contains("stream_keys AS MATERIALIZED"));
    assert!(payload.contains("projected_suffix AS MATERIALIZED"));
    assert_eq!(
        payload
            .matches("FROM telemetry_projected_raw_network_minutes_source($1::TEXT[]) suffix")
            .count(),
        1
    );
    assert!(payload.contains("candidate_points AS MATERIALIZED"));
    assert!(payload.contains("JOIN telemetry_network_rates retained"));
    assert!(payload.contains("JOIN traffic_counter_samples sample"));
    assert!(payload.contains("JOIN projected_suffix suffix"));
    assert!(payload.contains("LEFT JOIN LATERAL ("));
    assert!(payload.contains("FROM telemetry_network_rates predecessor"));
    assert!(payload.contains("FROM stream_keys stream"));
    assert!(payload.contains("FROM projected_suffix predecessor"));
    assert!(payload.contains("stream.first_unpromoted_observed_at IS NOT NULL"));
    assert!(payload.contains("sample.observed_at >="));
    assert!(payload.contains("stream.first_unpromoted_observed_at"));
    assert!(payload.contains("ORDER BY sample.observed_at DESC\n                LIMIT 1"));
    assert_eq!(
        payload
            .matches("predecessor.bucket_secs = candidate.bucket_secs")
            .count(),
        2
    );
    assert!(!cross_tier_payload.contains("predecessor.bucket_secs = candidate.bucket_secs"));
    assert!(payload.contains("AND candidate.bucket_secs = 60"));
    assert!(!cross_tier_payload.contains("AND candidate.bucket_secs = 60"));
    assert_eq!(payload.matches("FROM projected_suffix shadow").count(), 4);
    assert!(payload.contains("candidate.rx_counter_epoch = previous.rx_counter_epoch"));
    assert!(payload.contains("candidate.tx_counter_epoch = previous.tx_counter_epoch"));
    assert!(payload.contains("candidate.rx_bytes_last >= previous.rx_bytes_last"));
    assert!(payload.contains("candidate.tx_bytes_last >= previous.tx_bytes_last"));
    assert!(payload.contains("candidate.transition_valid"));
    assert!(payload.contains("candidate.ordinal AS candidate_ordinal"));
    assert!(payload.contains("ORDER BY candidate.ordinal ASC"));
    assert_eq!(
        payload
            .matches("candidate.latest_observed_at::text AS latest_observed_at")
            .count(),
        1
    );
    assert!(!payload.contains("visible_clients"));
    assert!(!payload.contains("OFFSET"));

    let source = include_str!("repository_telemetry_rollups.rs");
    let (_, function) = source
        .split_once("pub(crate) async fn list_telemetry_network_rates")
        .expect("raw network list function");
    let (function, _) = function
        .split_once("pub(crate) async fn list_latest_telemetry_network_rates")
        .expect("raw network list end");
    assert!(function.contains("raw_telemetry_network_rate_candidate_keys_sql("));
    assert!(function.contains("raw_telemetry_network_rate_payload_sql("));
    assert!(function.contains("sqlx::query(&candidate_sql)"));
    assert!(function.contains("sqlx::query(&payload_sql)"));
    assert!(function.contains("REPEATABLE READ, READ ONLY"));
    assert!(
        function.contains("SELECT client.id\n                        FROM visible_clients client")
    );
    assert!(function.contains("while result.len() < requested"));
    assert!(function.contains("let page_limit = requested"));
    assert!(function.contains("let mut next_cursor = None"));
    assert!(function.contains("row.try_get::<bool, _>(\"admitted\")"));
    assert!(function.contains("page_cursor_strictly_after"));
    assert!(function.contains("payload_rows.len() != candidate_keys.len()"));
    assert!(function.contains("candidate_ordinal"));
    assert!(function.contains("row.try_get::<bool, _>(\"transition_valid\")"));
    assert!(!function.contains("OFFSET"));
}

#[test]
fn dashboard_start_uses_projected_all_telemetry_minima() {
    let source = include_str!("repository_telemetry_rollups.rs");
    let (_, function) = source
        .split_once("pub(crate) async fn dashboard_telemetry_start_unix")
        .expect("dashboard telemetry start function");
    let (function, _) = function
        .split_once("pub(crate) async fn list_projected_telemetry_resource_history")
        .expect("next repository function");

    for field in [
        "resource_first_at",
        "network_first_at",
        "traffic_first_at",
        "ping_first_at",
    ] {
        assert!(function.contains(field));
    }
    assert!(function.contains("dashboard.resource_generation > 0"));
    assert!(function.contains("dashboard.network_generation > 0"));
    assert!(function.contains("dashboard.traffic_generation > 0"));
    assert!(!function.contains("network_selection_hash"));
    assert!(function.contains("LEFT JOIN telemetry_dashboard_projection_heads"));
    assert!(function.contains("bool_and(projected.projection_ready)"));
    assert!(!function.contains("FROM telemetry_rollups"));
    assert!(!function.contains("FROM telemetry_network_rates"));
    assert!(!function.contains("FROM telemetry_ping_rollups"));
}

#[test]
fn raw_network_payload_consumers_require_stored_bits_and_current_policy() {
    let source = include_str!("repository_telemetry_rollups.rs");
    let (_, generic_raw) = source
        .split_once("pub(crate) async fn list_telemetry_samples")
        .expect("generic raw telemetry reader");
    let (generic_raw, remainder) = generic_raw
        .split_once("pub(crate) async fn list_dashboard_raw_telemetry_rollups")
        .expect("generic raw telemetry reader end");

    assert!(generic_raw.contains("sample.network_admission_mask"));
    assert!(generic_raw.contains("sample.tunnel_admission_mask"));
    assert_eq!(
        generic_raw
            .matches("public.telemetry_ordinal_admission_mask_is_exact(")
            .count(),
        2,
        "each raw vector shape is validated once before any ordinal is released"
    );
    assert_eq!(generic_raw.matches("get_bit(").count(), 4);
    assert_eq!(
        generic_raw
            .matches("public.telemetry_interface_is_admitted_resolved(")
            .count(),
        1,
        "resolved policy is evaluated over the page's distinct stamped names"
    );
    assert_eq!(
        generic_raw
            .matches("public.resolve_telemetry_interface_policies(")
            .count(),
        1,
        "policy relations are resolved once for the page"
    );
    assert!(!generic_raw.contains("public.telemetry_interface_is_admitted("));
    assert_eq!(
        generic_raw.matches("WITH ORDINALITY AS").count(),
        4,
        "both discovery and reconstruction must preserve reported ordinals"
    );
    assert!(generic_raw.contains(
        "WHEN network.ordinality <=\n                                                   octet_length("
    ));
    assert!(generic_raw.contains(
        "WHEN tunnel.ordinality <=\n                                               octet_length("
    ));
    assert!(!generic_raw.contains("COALESCE(sample.network_admission_mask"));
    assert!(!generic_raw.contains("COALESCE(sample.tunnel_admission_mask"));

    let (_, raw_dashboard) = remainder
        .split_once("async fn list_dashboard_raw_telemetry_network_rates_selected_with_output")
        .expect("raw dashboard host reader");
    let (raw_dashboard, _) = raw_dashboard
        .split_once("pub(crate) async fn dashboard_telemetry_start_unix")
        .expect("raw dashboard host reader end");
    assert!(raw_dashboard.contains("query_projected_telemetry_network_history("));
    assert!(raw_dashboard.contains("project_network_rate_selection("));
    assert!(!raw_dashboard.contains("get_bit("));
    assert!(!raw_dashboard.contains("telemetry_samples"));
}

#[test]
fn ordinal_admission_requires_one_exact_whole_vector_encoding() {
    fn visible(
        mask: &[u8],
        item_count: usize,
        ordinal: usize,
        current_policy_admits: bool,
    ) -> bool {
        vpsman_common::ordinal_admission_mask_has_exact_shape(mask, item_count)
            && mask
                .get(ordinal / 8)
                .is_some_and(|byte| byte & (1_u8 << (ordinal % 8)) != 0)
            && current_policy_admits
    }

    // Nine items need exactly two bytes and only bit zero may be set in the
    // second byte. A short mask never exposes its otherwise valid-looking
    // eight-item prefix, and non-zero unused high bits invalidate the vector.
    assert!(!visible(&[0xff], 9, 0, true));
    assert!(!visible(&[0xff, 0b0000_0011], 9, 0, true));
    assert!(!visible(&[0xff, 0b0000_0001, 0], 9, 0, true));
    let admitted_at_acceptance = [0b0000_0001, 0b0000_0001];
    assert!(visible(&admitted_at_acceptance, 9, 0, true));
    assert!(visible(&admitted_at_acceptance, 9, 8, true));
    assert!(!visible(&admitted_at_acceptance, 9, 7, true));
    assert!(!visible(&admitted_at_acceptance, 9, 9, true));

    // Widening today's rule cannot resurrect a sample rejected when accepted.
    assert!(!visible(&[0], 1, 0, true));
    // Narrowing today's rule hides a formerly admitted sample immediately.
    assert!(!visible(&[1], 1, 0, false));

    let migration = include_str!("../../../../../migrations/0003_telemetry_core.sql");
    assert!(migration.contains("CREATE FUNCTION public.telemetry_ordinal_admission_mask_is_exact"));
    assert!(migration.contains("WHEN octet_length(p_mask)::BIGINT <>"));
    assert!(migration.contains("get_byte(p_mask, octet_length(p_mask) - 1)"));
}

#[test]
fn bounded_network_history_preserves_canonical_stream_and_predecessor_semantics() {
    let schema = include_str!("../../../../../migrations/0003_telemetry_core.sql");
    let (_, durable) = schema
        .split_once("CREATE FUNCTION public.telemetry_network_durable_points_source(")
        .expect("durable network source");
    let (durable, _) = durable
        .split_once("CREATE FUNCTION public.telemetry_network_rate_points_source(")
        .expect("durable network source boundary");
    assert!(durable.contains("p_per_stream_limit BIGINT DEFAULT NULL"));
    assert!(durable.contains("IF p_per_stream_limit IS NOT NULL THEN"));
    assert!(durable.contains("p_client_ids IS NULL OR p_interfaces IS NULL"));
    assert!(durable.contains("cardinality(p_client_ids) <> cardinality(p_interfaces)"));
    assert!(durable.contains("array_position(p_client_ids, NULL) IS NOT NULL"));
    assert!(durable.contains("array_position(p_interfaces, NULL) IS NOT NULL"));
    assert!(durable.contains("FROM unnest(p_client_ids, p_interfaces)"));
    assert!(durable.contains("requested_streams AS MATERIALIZED"));
    for owner in [
        "FROM public.telemetry_network_rates_minute minute",
        "FROM public.traffic_counter_samples sample",
        "FROM public.telemetry_network_rates_coarse coarse",
    ] {
        assert!(durable.contains(owner), "bounded stream omits {owner}");
    }
    assert!(durable.contains("sample.source_kind = 'host'"));
    assert!(durable.contains("NOT sample.inbound_promoted"));
    assert!(schema.contains(
        "telemetry_network_rates_minute_client_effective_idx ON public.telemetry_network_rates_minute USING btree (client_id, interface, latest_observed_at DESC, bucket_start DESC)"
    ));
    assert!(schema.contains(
        "telemetry_network_rates_coarse_client_effective_idx ON public.telemetry_network_rates_coarse USING btree (client_id, interface, latest_observed_at DESC, bucket_start DESC, bucket_secs DESC)"
    ));
    assert!(schema.contains(
        "CONSTRAINT traffic_counter_samples_pkey PRIMARY KEY (client_id, source_kind, interface, observed_at)"
    ));
    assert!(durable.contains("minute.latest_observed_at >= p_min_bucket_start"));
    assert!(durable
        .contains("minute.latest_observed_at\n                              < p_max_bucket_start + interval '1 minute'"));
    assert!(durable.contains("coarse.latest_observed_at >= p_min_bucket_start"));
    assert!(durable
        .contains("coarse.latest_observed_at\n                              < p_max_bucket_start + interval '1 day'"));
    assert!(durable.contains("ORDER BY sample.observed_at DESC"));
    assert!(durable.contains(
        "ORDER BY candidate.bucket_start DESC,\n                     candidate.latest_observed_at DESC"
    ));
    assert!(durable.contains("LIMIT p_per_stream_limit\n            OFFSET 0"));

    let sql = TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL;
    let stream_arrays = sql
        .find("selected_stream_arrays AS MATERIALIZED")
        .expect("paired selected-stream arrays");
    let durable_call = sql[stream_arrays..]
        .find("CROSS JOIN LATERAL telemetry_network_durable_points_source(")
        .map(|offset| stream_arrays + offset)
        .expect("one set-wise durable source call");
    let shadow = sql[durable_call..]
        .find("FROM projected_suffix shadow")
        .map(|offset| durable_call + offset)
        .expect("projected shadow");
    let suffix = sql[shadow..]
        .find("SELECT suffix.*")
        .map(|offset| shadow + offset)
        .expect("projected suffix merge");
    let outer_cap = sql[suffix..]
        .find("WHERE ranked.physical_rank <= ($5::BIGINT + 1)")
        .map(|offset| suffix + offset)
        .expect("post-overlay per-stream physical cap");
    let native_grouping = sql.find("bucket_values AS").expect("native chart grouping");
    assert!(stream_arrays < durable_call && durable_call < shadow);
    assert!(shadow < suffix && suffix < outer_cap && outer_cap < native_grouping);
    assert_eq!(
        sql.matches("($5::BIGINT + 1)").count(),
        2,
        "durable and post-overlay caps must use the same N+1 formula"
    );
    assert_eq!(
        sql.matches("telemetry_network_durable_points_source(")
            .count(),
        1,
        "network history must initialize one durable executor per request"
    );
    assert!(!sql.contains("ARRAY[stream.client_id]"));
    assert!(!sql.contains("ARRAY[stream.interface]"));
    assert_eq!(
        sql.matches("FROM telemetry_projected_raw_network_minutes_source($1::TEXT[])")
            .count(),
        1,
        "the cap must not repeat projected JSON expansion per interface"
    );
    assert!(sql.contains("point.recency_rank = $5 + 1"));
    assert!(sql.contains("missing_predecessor_keys AS MATERIALIZED"));
    assert!(sql.contains("retained.latest_observed_at < oldest.first_source_start"));

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Point {
        start: i64,
        bucket_secs: i64,
        latest: i64,
        counter: i64,
        epoch: i64,
    }

    #[derive(Clone)]
    struct Bucket {
        chart_start: i64,
        effective_step: i64,
        first_source_start: i64,
        terminal: Point,
    }

    fn canonical_points(durable: &[Point], projected: &[Point]) -> Vec<Point> {
        durable
            .iter()
            .filter(|point| {
                !projected.iter().any(|shadow| {
                    shadow.start == point.start && shadow.bucket_secs == point.bucket_secs
                })
            })
            .chain(projected)
            .cloned()
            .collect()
    }

    fn oracle(
        durable: &[Point],
        projected: &[Point],
        start: i64,
        end: i64,
        step: i64,
        points: usize,
        physical_cap: Option<usize>,
    ) -> Vec<(i64, i64)> {
        use std::collections::BTreeMap;

        let canonical = canonical_points(durable, projected);
        let mut expanded = canonical
            .iter()
            .filter(|point| point.start >= start - 86_400 && point.start <= end)
            .cloned()
            .collect::<Vec<_>>();
        expanded.sort_unstable_by_key(|point| {
            (
                std::cmp::Reverse(point.start),
                std::cmp::Reverse(point.latest),
                point.bucket_secs,
            )
        });
        if let Some(cap) = physical_cap {
            expanded.truncate(cap);
        }

        let mut grouped = BTreeMap::<(i64, i64), Bucket>::new();
        for point in expanded
            .into_iter()
            .filter(|point| point.start + point.bucket_secs > start)
        {
            let effective_step = step.max(point.bucket_secs);
            let chart_start = point.start.div_euclid(effective_step) * effective_step;
            grouped
                .entry((chart_start, effective_step))
                .and_modify(|bucket| {
                    bucket.first_source_start = bucket.first_source_start.min(point.start);
                    if (point.latest, point.start, point.bucket_secs)
                        > (
                            bucket.terminal.latest,
                            bucket.terminal.start,
                            bucket.terminal.bucket_secs,
                        )
                    {
                        bucket.terminal = point.clone();
                    }
                })
                .or_insert(Bucket {
                    chart_start,
                    effective_step,
                    first_source_start: point.start,
                    terminal: point,
                });
        }
        let mut ranked = grouped.into_values().collect::<Vec<_>>();
        ranked.sort_unstable_by_key(|bucket| {
            (
                std::cmp::Reverse(bucket.chart_start),
                std::cmp::Reverse(bucket.effective_step),
            )
        });
        let predecessor = ranked
            .get(points)
            .map(|bucket| bucket.terminal.clone())
            .or_else(|| {
                ranked.last().and_then(|oldest| {
                    canonical
                        .iter()
                        .filter(|point| point.latest < oldest.first_source_start)
                        .max_by_key(|point| (point.latest, point.start, point.bucket_secs))
                        .cloned()
                })
            });
        ranked.truncate(points);
        let mut states = predecessor
            .into_iter()
            .map(|point| (None, point))
            .chain(
                ranked
                    .into_iter()
                    .map(|bucket| (Some(bucket.chart_start), bucket.terminal)),
            )
            .collect::<Vec<_>>();
        states.sort_unstable_by_key(|(_, point)| point.latest);
        states
            .windows(2)
            .filter_map(|pair| {
                let (chart_start, current) = &pair[1];
                let previous = &pair[0].1;
                chart_start.and_then(|chart_start| {
                    (current.latest > previous.latest
                        && current.epoch == previous.epoch
                        && current.counter >= previous.counter)
                        .then_some((chart_start, current.counter - previous.counter))
                })
            })
            .collect()
    }

    fn point(start: i64, bucket_secs: i64, counter: i64, epoch: i64) -> Point {
        Point {
            start,
            bucket_secs,
            latest: start + bucket_secs - 1,
            counter,
            epoch,
        }
    }

    let base = 12 * 86_400_i64;
    let dense = (-1_440..=15)
        .map(|minute| point(base + minute * 60, 60, 10_000 + minute, 0))
        .collect::<Vec<_>>();
    let sparse = [-2_880, -14, -9, -3, 0, 8, 15]
        .into_iter()
        .map(|minute| point(base + minute * 60, 60, 20_000 + minute, 0))
        .collect::<Vec<_>>();
    let mixed = [
        point(base - 300, 300, 30_000, 0),
        point(base, 300, 30_500, 0),
        point(base + 300, 60, 30_600, 0),
        point(base + 360, 60, 30_700, 0),
        point(base + 420, 60, 30_800, 0),
        point(base + 480, 60, 30_900, 0),
        point(base + 540, 60, 31_000, 0),
    ];
    let reset = [
        point(base - 60, 60, 100, 0),
        point(base, 60, 200, 0),
        point(base + 60, 60, 5, 1),
        point(base + 120, 60, 25, 1),
        point(base + 180, 60, 45, 1),
    ];
    let n_plus_one = (-1..=5)
        .map(|minute| point(base + minute * 60, 60, 40_000 + minute * 100, 0))
        .collect::<Vec<_>>();
    let missing_predecessor = [
        point(base - 2 * 86_400, 60, 50_000, 0),
        point(base, 60, 50_100, 0),
        point(base + 60, 60, 50_200, 0),
    ];
    let daily = (-1..=4)
        .map(|day| point(base + day * 86_400, 86_400, 60_000 + day * 1_000, 0))
        .collect::<Vec<_>>();
    let shadow_durable = [
        point(base - 60, 60, 50, 0),
        point(base, 60, 100, 0),
        point(base + 60, 60, 200, 0),
    ];
    let shadow_projected = [point(base, 60, 150, 0)];

    type HistoryCase<'a> = (&'a str, &'a [Point], &'a [Point], i64, i64, i64, usize);

    let cases: [HistoryCase<'_>; 8] = [
        ("dense recent", &dense, &[], base, base + 900, 60, 16),
        ("sparse gaps", &sparse, &[], base, base + 900, 60, 16),
        ("mixed tier", &mixed, &[], base, base + 599, 300, 3),
        ("reset epoch", &reset, &[], base, base + 180, 60, 3),
        ("rank N+1", &n_plus_one, &[], base, base + 300, 60, 3),
        (
            "missing predecessor fallback",
            &missing_predecessor,
            &[],
            base,
            base + 60,
            60,
            2,
        ),
        ("old daily", &daily, &[], base, base + 4 * 86_400, 86_400, 3),
        (
            "projected shadow",
            &shadow_durable,
            &shadow_projected,
            base,
            base + 60,
            60,
            2,
        ),
    ];
    for (name, durable, projected, start, end, step, points) in cases {
        let rows_per_chart_point = usize::try_from((step + 59) / 60).unwrap();
        let cap = (points + 1).checked_mul(rows_per_chart_point).unwrap();
        assert_eq!(
            oracle(durable, projected, start, end, step, points, Some(cap)),
            oracle(durable, projected, start, end, step, points, None),
            "bounded canonical projection diverged for {name}"
        );
    }
}
