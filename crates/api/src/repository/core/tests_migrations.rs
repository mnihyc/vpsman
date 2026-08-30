use uuid::Uuid;

use crate::{
    model::CreateMigrationLinkRequest, repository::API_POSTGRES_SESSION_OPTIONS,
    routes_migrations::validate_create_migration_link,
};

#[test]
fn latency_facing_api_sessions_disable_jit_without_changing_migration_options() {
    assert_eq!(
        API_POSTGRES_SESSION_OPTIONS,
        [("search_path", "public"), ("jit", "off")]
    );

    let repository = include_str!("repository.rs");
    let (_, connect) = repository
        .split_once("pub(crate) async fn connect(")
        .expect("API repository connection owner");
    let (connect, migration) = connect
        .split_once("pub(crate) async fn migrate_postgres_database(")
        .expect("dedicated migration connection boundary");
    assert!(connect.contains("migrate_postgres_database(&connect_options, migrations_dir)"));
    assert!(connect.contains(".options(API_POSTGRES_SESSION_OPTIONS)"));
    assert!(migration.contains("PgConnection::connect_with(connect_options)"));
    assert!(!migration.contains(".options(API_POSTGRES_SESSION_OPTIONS)"));
}

#[test]
fn migration_link_validation_requires_confirmation() {
    let unconfirmed = CreateMigrationLinkRequest {
        restore_plan_id: Uuid::new_v4(),
        confirmed: false,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_migration_link(&unconfirmed)
            .unwrap_err()
            .code,
        "migration_confirmation_required"
    );

    let oversized_note = CreateMigrationLinkRequest {
        restore_plan_id: Uuid::new_v4(),
        confirmed: true,
        note: Some("x".repeat(1025)),
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_migration_link(&oversized_note)
            .unwrap_err()
            .code,
        "migration_note_too_long"
    );
}

#[test]
fn backup_latest_lists_have_matching_global_order_indexes() {
    let migration = include_str!("../../../../../migrations/0007_backups_restores.sql")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    assert!(migration.contains(
        "CREATE INDEX backup_artifacts_created_idx ON public.backup_artifacts USING btree (created_at DESC, id DESC);"
    ));
    assert!(migration.contains(
        "CREATE INDEX backup_requests_created_idx ON public.backup_requests USING btree (created_at DESC, id DESC);"
    ));
}

#[test]
fn dashboard_relations_share_one_immutable_client_ownership_root() {
    let migration = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");
    let root_declaration = "CREATE TABLE public.telemetry_dashboard_clients (";
    let first_child_declaration =
        "CREATE TABLE public.telemetry_dashboard_resource_projection_heads (";
    let root_declaration = migration
        .find(root_declaration)
        .expect("dashboard client root declaration");
    let first_child_declaration = migration
        .find(first_child_declaration)
        .expect("dashboard first child declaration");
    assert!(
        root_declaration < first_child_declaration,
        "the dashboard client root must exist before every dependent relation"
    );
    assert_eq!(
        migration
            .matches("REFERENCES public.clients(id) ON DELETE CASCADE")
            .count(),
        1,
        "only the immutable dashboard root may reference the mutable client tuple"
    );

    let direct_client_relations = [
        "telemetry_dashboard_resource_projection_heads",
        "telemetry_dashboard_network_generations",
        "telemetry_dashboard_traffic_generations",
        "telemetry_dashboard_network_projection_heads",
        "telemetry_dashboard_traffic_projection_heads",
        "telemetry_dashboard_ping_projection_heads",
        "telemetry_dashboard_projection_fences",
        "telemetry_dashboard_block_events",
        "telemetry_dashboard_generation_events",
        "telemetry_dashboard_resource_generation_bounds",
        "telemetry_dashboard_resource_blocks",
    ];
    assert_eq!(
        migration
            .matches("REFERENCES public.telemetry_dashboard_clients(client_id)")
            .count(),
        direct_client_relations.len()
    );
    for relation in direct_client_relations {
        let declaration = format!("CREATE TABLE public.{relation} (");
        let body = migration
            .split_once(&declaration)
            .unwrap_or_else(|| panic!("missing dashboard relation {relation}"))
            .1
            .split_once("\n);\n")
            .unwrap_or_else(|| panic!("unterminated dashboard relation {relation}"))
            .0;
        assert!(
            body.contains("REFERENCES public.telemetry_dashboard_clients(client_id)")
                && body.contains("ON DELETE CASCADE"),
            "dashboard relation {relation} bypasses the stable client root"
        );
    }

    let initializer = migration
        .split_once("CREATE FUNCTION public.initialize_telemetry_dashboard_client()")
        .expect("dashboard client initializer")
        .1
        .split_once("CREATE TRIGGER clients_telemetry_dashboard_initialize")
        .expect("dashboard client initializer boundary")
        .0;
    let root_insert = initializer
        .find("INSERT INTO public.telemetry_dashboard_clients")
        .expect("dashboard client root initialization");
    let first_child_insert = initializer
        .find("INSERT INTO public.telemetry_dashboard_resource_projection_heads")
        .expect("dashboard resource-head initialization");
    assert!(root_insert < first_child_insert);
    assert!(!migration.contains("UPDATE public.telemetry_dashboard_clients"));
    assert!(!migration.contains("DELETE FROM public.telemetry_dashboard_clients"));
}

#[test]
fn dashboard_source_delete_work_requires_a_live_dashboard_owner() {
    let migration = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");
    for function_name in [
        "queue_telemetry_resource_blocks_after_delete",
        "queue_telemetry_network_blocks_after_delete",
        "maintain_telemetry_ping_series_dashboard_after_delete",
        "maintain_telemetry_ping_dashboard_after_delete",
    ] {
        let declaration = format!("CREATE FUNCTION public.{function_name}()");
        let body = migration
            .split_once(&declaration)
            .unwrap_or_else(|| panic!("missing dashboard delete function {function_name}"))
            .1
            .split_once("\n$$;\n")
            .unwrap_or_else(|| panic!("unterminated dashboard delete function {function_name}"))
            .0;
        assert!(
            body.contains("JOIN public.telemetry_dashboard_clients dashboard_client"),
            "dashboard delete function {function_name} can emit work after owner removal"
        );
    }
}

#[test]
fn dashboard_owner_acquisition_scans_one_row_per_ready_owner_without_a_scan_cap() {
    let migration = include_str!("../../../../../migrations/0006_telemetry_dashboard.sql");
    let acquisition = migration
        .split_once("CREATE FUNCTION public.acquire_next_telemetry_dashboard_projection_owner()")
        .expect("dashboard owner acquisition function")
        .1
        .split_once("CREATE FUNCTION public.claim_telemetry_dashboard_projection(")
        .expect("dashboard owner acquisition boundary")
        .0;

    let ready_scan = acquisition
        .find("FROM public.telemetry_dashboard_ready_owners ready")
        .expect("bounded ready-owner scan");
    let advisory_probe = acquisition
        .find("IF pg_try_advisory_lock(candidate.owner_id)")
        .expect("owner advisory probe");
    let fence_lookup = acquisition
        .find("FROM public.telemetry_dashboard_projection_fences fence")
        .expect("post-lock stable owner fence lookup");

    assert!(ready_scan < advisory_probe && advisory_probe < fence_lookup);
    assert!(
        acquisition.contains("WHERE ready.retry_not_before <= clock_timestamp()"),
        "a failed owner must leave the due set without blocking later owners"
    );
    assert!(acquisition.contains("ORDER BY ready.ready_at, ready.owner_id"));
    assert!(acquisition.contains("WHERE fence.owner_id = candidate.owner_id"));
    assert!(acquisition.contains("PERFORM pg_advisory_unlock(candidate.owner_id)"));
    assert!(acquisition.contains("ready_revision := candidate.wake_revision"));
    assert!(!acquisition.contains("telemetry_dashboard_block_events"));
    assert!(!acquisition.contains("telemetry_dashboard_generation_events"));
    assert!(!acquisition.contains("seen_resource_client_ids"));
    assert!(!acquisition.contains("seen_network_client_ids"));
    assert!(
        !acquisition.contains("LIMIT "),
        "owner acquisition must scan past every currently locked ready owner"
    );

    assert!(migration.contains("CREATE TABLE public.telemetry_dashboard_ready_owners ("));
    assert!(migration.contains("CREATE INDEX telemetry_dashboard_ready_owners_fifo_idx"));
    assert!(migration.contains("DEFAULT '-infinity'::TIMESTAMPTZ"));
    assert!(migration.contains("retry_not_before = '-infinity'::TIMESTAMPTZ"));
    assert_eq!(
        migration
            .matches("EXECUTE FUNCTION public.mark_telemetry_dashboard_owners_ready();")
            .count(),
        2,
        "both immutable event relations must maintain the same ready-owner boundary"
    );
    assert!(!migration.contains("telemetry_dashboard_block_events_fifo_idx"));
    assert!(!migration.contains("telemetry_dashboard_generation_events_fifo_idx"));

    let claim = migration
        .split_once("CREATE FUNCTION public.claim_telemetry_dashboard_projection(")
        .expect("dashboard owner claim function")
        .1
        .split_once("CREATE FUNCTION public.publish_telemetry_dashboard_projection(")
        .expect("dashboard owner claim boundary")
        .0;
    for (relation, index) in [
        (
            "public.telemetry_dashboard_block_events event",
            "telemetry_dashboard_block_events_owner_event_idx",
        ),
        (
            "public.telemetry_dashboard_generation_events event",
            "telemetry_dashboard_generation_events_owner_event_idx",
        ),
    ] {
        assert!(claim.contains(relation));
        assert!(claim.contains("event.client_id = locked.client_id"));
        assert!(claim.contains("event.domain = locked.domain"));
        assert!(migration.contains(&format!("CREATE INDEX {index}")));
    }
}
