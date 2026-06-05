use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, post, put},
    Router,
};

use crate::{
    routes_alerts::{
        dispatch_fleet_alert_notifications, export_fleet_alerts,
        list_fleet_alert_notification_channels, list_fleet_alert_notifications,
        list_fleet_alert_policies, list_fleet_alert_states, list_fleet_alerts,
        process_fleet_alert_notifications, update_fleet_alert_state,
        upsert_fleet_alert_notification_channel, upsert_fleet_alert_policy,
    },
    routes_auth::{
        bootstrap_operator, confirm_operator_totp, create_operator, current_operator,
        disable_operator_totp, list_operator_sessions, list_operators, login_operator,
        refresh_operator_session, revoke_operator_session, setup_operator_totp,
        update_operator_preferences,
    },
    routes_backups::{
        abort_backup_artifact_upload_session, commit_backup_artifact_upload_session,
        create_backup_artifact_handoff, create_backup_artifact_upload_session,
        create_backup_policy, create_backup_request, download_backup_artifact,
        list_backup_artifacts, list_backup_policies, list_backup_requests,
        prepare_backup_artifact_restore, prune_backup_policies, record_backup_artifact_metadata,
        upload_backup_artifact, upload_backup_artifact_session_chunk,
        MAX_BACKUP_ARTIFACT_UPLOAD_BODY_BYTES,
    },
    routes_command_templates::{list_command_templates, upsert_command_template},
    routes_dashboard::dashboard_overview,
    routes_discovery::discovery_endpoints,
    routes_enrollment::{
        claim_enrollment, create_enrollment_token, key_lifecycle_report,
        list_client_key_revocations, list_enrollment_tokens, revoke_current_client_key,
    },
    routes_file_transfers::{
        create_file_transfer_handoff, download_file_transfer_handoff,
        download_file_transfer_source_artifact, list_file_transfer_sessions,
        list_file_transfer_source_artifacts, upload_file_transfer_source_artifact,
        MAX_FILE_TRANSFER_SOURCE_UPLOAD_BODY_BYTES,
    },
    routes_history::{
        export_history, list_history_retention_policies, prune_history_retention,
        upsert_history_retention_policy,
    },
    routes_ingest::{
        ingest_agent_hello, ingest_command_output, ingest_gateway_session_ended,
        ingest_gateway_session_started, ingest_telemetry, ingest_terminal_output,
        validate_agent_identity,
    },
    routes_inventory::{
        assign_agent_tag, assign_data_source_preset, clone_data_source_preset,
        create_data_source_preset, create_tag, delete_agent, diff_data_source_preset,
        fleet_summary, list_agents, list_data_source_assignments, list_data_source_presets,
        list_data_source_status, list_gateway_sessions, list_tags, list_telemetry_network_rates,
        list_telemetry_rollups, list_telemetry_tunnels, render_data_source_hot_config,
        resolve_bulk_targets, test_data_source_preset, update_agent_alias,
        update_data_source_preset,
    },
    routes_job_history::{
        compare_job_outputs, download_job_output_artifact, get_job, list_audit_logs,
        list_auth_proof_rotations, list_job_outputs, list_job_targets, list_jobs,
        list_network_observation_trends, list_network_observations,
        list_process_supervisor_inventory,
    },
    routes_jobs::{cancel_job, create_job, dispatch_scheduled_job},
    routes_migrations::{create_migration_link, list_migration_links},
    routes_network::{
        create_tunnel_plan, get_topology_graph, list_network_ospf_recommendations,
        list_network_ospf_update_plans, list_tunnel_plans, promote_telemetry_tunnel_plan,
        promote_tunnel_plan_to_adapter,
    },
    routes_restores::{create_restore_plan, list_restore_plans},
    routes_rollout_policies::{
        create_agent_update_rollout_policy, list_agent_update_rollout_policies,
    },
    routes_rollouts::{
        list_agent_update_rollouts, record_agent_update_activation_delegation,
        record_agent_update_rollback_delegation, update_agent_update_rollout_control,
    },
    routes_schedules::{create_schedule, list_schedules},
    routes_terminal_sessions::{list_terminal_sessions, terminal_session_replay},
    routes_update_releases::{
        create_agent_update_release, create_hosted_agent_update_release,
        download_agent_update_artifact, latest_agent_update_release, list_agent_update_releases,
        stream_agent_update_artifact, upload_agent_update_artifact,
        MAX_RELEASE_ARTIFACT_UPLOAD_BODY_BYTES,
    },
    routes_ws::ws_handler,
    state::AppState,
};

pub(crate) fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route(
            "/.well-known/vpsman/endpoints.json",
            get(discovery_endpoints),
        )
        .route("/api/v1/discovery/endpoints", get(discovery_endpoints))
        .route("/api/v1/auth/bootstrap", post(bootstrap_operator))
        .route("/api/v1/auth/login", post(login_operator))
        .route("/api/v1/auth/refresh", post(refresh_operator_session))
        .route("/api/v1/auth/me", get(current_operator))
        .route("/api/v1/auth/preferences", put(update_operator_preferences))
        .route("/api/v1/auth/totp/setup", post(setup_operator_totp))
        .route("/api/v1/auth/totp/confirm", post(confirm_operator_totp))
        .route("/api/v1/auth/totp/disable", post(disable_operator_totp))
        .route(
            "/api/v1/auth/proof-rotations",
            get(list_auth_proof_rotations),
        )
        .route(
            "/api/v1/operators",
            get(list_operators).post(create_operator),
        )
        .route("/api/v1/operator-sessions", get(list_operator_sessions))
        .route(
            "/api/v1/operator-sessions/{session_id}",
            delete(revoke_operator_session),
        )
        .route(
            "/api/v1/enrollment-tokens",
            get(list_enrollment_tokens).post(create_enrollment_token),
        )
        .route("/api/v1/enrollments/claim", post(claim_enrollment))
        .route(
            "/api/v1/client-key-revocations",
            get(list_client_key_revocations),
        )
        .route(
            "/api/v1/clients/{client_id}/key-revocations",
            post(revoke_current_client_key),
        )
        .route("/api/v1/key-lifecycle/report", get(key_lifecycle_report))
        .route("/api/v1/dashboard/overview", get(dashboard_overview))
        .route("/api/v1/fleet/summary", get(fleet_summary))
        .route("/api/v1/fleet-alerts", get(list_fleet_alerts))
        .route("/api/v1/fleet-alerts/export", get(export_fleet_alerts))
        .route(
            "/api/v1/fleet-alert-states",
            get(list_fleet_alert_states).post(update_fleet_alert_state),
        )
        .route(
            "/api/v1/fleet-alert-policies",
            get(list_fleet_alert_policies).post(upsert_fleet_alert_policy),
        )
        .route(
            "/api/v1/fleet-alert-notification-channels",
            get(list_fleet_alert_notification_channels)
                .post(upsert_fleet_alert_notification_channel),
        )
        .route(
            "/api/v1/fleet-alert-notifications",
            get(list_fleet_alert_notifications),
        )
        .route(
            "/api/v1/fleet-alert-notifications/dispatch",
            post(dispatch_fleet_alert_notifications),
        )
        .route(
            "/api/v1/fleet-alert-notifications/process",
            post(process_fleet_alert_notifications),
        )
        .route("/api/v1/agents", get(list_agents))
        .route("/api/v1/agents/{client_id}/delete", post(delete_agent))
        .route("/api/v1/gateway-sessions", get(list_gateway_sessions))
        .route("/api/v1/telemetry/rollups", get(list_telemetry_rollups))
        .route(
            "/api/v1/telemetry/network-rates",
            get(list_telemetry_network_rates),
        )
        .route("/api/v1/telemetry/tunnels", get(list_telemetry_tunnels))
        .route(
            "/api/v1/history/retention-policies",
            get(list_history_retention_policies).post(upsert_history_retention_policy),
        )
        .route(
            "/api/v1/history/retention-prune",
            post(prune_history_retention),
        )
        .route("/api/v1/history/export", get(export_history))
        .route("/api/v1/tags", get(list_tags).post(create_tag))
        .route(
            "/api/v1/data-source-presets",
            get(list_data_source_presets).post(create_data_source_preset),
        )
        .route(
            "/api/v1/data-source-presets/{preset_id}/clone",
            post(clone_data_source_preset),
        )
        .route(
            "/api/v1/data-source-presets/{preset_id}/diff",
            post(diff_data_source_preset),
        )
        .route(
            "/api/v1/data-source-presets/{preset_id}/test",
            post(test_data_source_preset),
        )
        .route(
            "/api/v1/data-source-presets/{preset_id}/update",
            post(update_data_source_preset),
        )
        .route(
            "/api/v1/data-source-assignments",
            get(list_data_source_assignments).post(assign_data_source_preset),
        )
        .route("/api/v1/data-source-status", get(list_data_source_status))
        .route(
            "/api/v1/data-source-hot-config",
            get(render_data_source_hot_config),
        )
        .route("/api/v1/agents/{client_id}/tags", post(assign_agent_tag))
        .route("/api/v1/agents/{client_id}/alias", post(update_agent_alias))
        .route("/api/v1/bulk/resolve", post(resolve_bulk_targets))
        .route("/api/v1/jobs", get(list_jobs).post(create_job))
        .route(
            "/api/v1/command-templates",
            get(list_command_templates).post(upsert_command_template),
        )
        .route(
            "/api/v1/agent-update-rollouts",
            get(list_agent_update_rollouts),
        )
        .route(
            "/api/v1/agent-update-rollout-policies",
            get(list_agent_update_rollout_policies).post(create_agent_update_rollout_policy),
        )
        .route(
            "/api/v1/agent-update-rollouts/{rollout_id}/control",
            post(update_agent_update_rollout_control),
        )
        .route(
            "/api/v1/agent-update-rollouts/{rollout_id}/rollback-delegation",
            post(record_agent_update_rollback_delegation),
        )
        .route(
            "/api/v1/agent-update-rollouts/{rollout_id}/activation-delegation",
            post(record_agent_update_activation_delegation),
        )
        .route(
            "/api/v1/agent-update-releases",
            get(list_agent_update_releases).post(create_agent_update_release),
        )
        .route(
            "/api/v1/agent-update-releases/latest",
            get(latest_agent_update_release),
        )
        .route(
            "/api/v1/agent-update-releases/upload",
            post(upload_agent_update_artifact).layer(DefaultBodyLimit::max(
                MAX_RELEASE_ARTIFACT_UPLOAD_BODY_BYTES,
            )),
        )
        .route(
            "/api/v1/agent-update-releases/hosted",
            post(create_hosted_agent_update_release),
        )
        .route(
            "/api/v1/agent-update-artifacts/stream",
            post(stream_agent_update_artifact).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/api/v1/agent-update-artifacts/{artifact_sha256_hex}",
            get(download_agent_update_artifact),
        )
        .route("/api/v1/jobs/{job_id}", get(get_job))
        .route("/api/v1/jobs/{job_id}/cancel", post(cancel_job))
        .route(
            "/api/v1/jobs/{job_id}/dispatch-scheduled",
            post(dispatch_scheduled_job),
        )
        .route("/api/v1/jobs/{job_id}/targets", get(list_job_targets))
        .route("/api/v1/jobs/{job_id}/outputs", get(list_job_outputs))
        .route(
            "/api/v1/jobs/{job_id}/output-comparison",
            get(compare_job_outputs),
        )
        .route(
            "/api/v1/jobs/{job_id}/outputs/{client_id}/{seq}/artifact",
            get(download_job_output_artifact),
        )
        .route(
            "/api/v1/process-supervisor/inventory",
            get(list_process_supervisor_inventory),
        )
        .route("/api/v1/file-transfers", get(list_file_transfer_sessions))
        .route(
            "/api/v1/file-transfer-sources",
            get(list_file_transfer_source_artifacts)
                .post(upload_file_transfer_source_artifact)
                .layer(DefaultBodyLimit::max(
                    MAX_FILE_TRANSFER_SOURCE_UPLOAD_BODY_BYTES,
                )),
        )
        .route(
            "/api/v1/file-transfer-sources/{artifact_id}/artifact",
            get(download_file_transfer_source_artifact),
        )
        .route(
            "/api/v1/file-transfers/{client_id}/{session_id}/handoff",
            post(create_file_transfer_handoff),
        )
        .route(
            "/api/v1/file-transfers/{client_id}/{session_id}/handoff/artifact",
            get(download_file_transfer_handoff),
        )
        .route("/api/v1/terminal-sessions", get(list_terminal_sessions))
        .route(
            "/api/v1/terminal-sessions/{client_id}/{session_id}/replay",
            get(terminal_session_replay),
        )
        .route(
            "/api/v1/network/observations",
            get(list_network_observations),
        )
        .route(
            "/api/v1/network/observation-trends",
            get(list_network_observation_trends),
        )
        .route(
            "/api/v1/network/ospf-recommendations",
            get(list_network_ospf_recommendations),
        )
        .route(
            "/api/v1/network/ospf-update-plans",
            get(list_network_ospf_update_plans),
        )
        .route("/api/v1/network/topology-graph", get(get_topology_graph))
        .route(
            "/api/v1/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/v1/tunnel-plans",
            get(list_tunnel_plans).post(create_tunnel_plan),
        )
        .route(
            "/api/v1/tunnel-plans/promote-telemetry",
            post(promote_telemetry_tunnel_plan),
        )
        .route(
            "/api/v1/tunnel-plans/promote-adapter",
            post(promote_tunnel_plan_to_adapter),
        )
        .route(
            "/api/v1/backups",
            get(list_backup_requests).post(create_backup_request),
        )
        .route("/api/v1/backup-artifacts", get(list_backup_artifacts))
        .route(
            "/api/v1/backup-policies",
            get(list_backup_policies).post(create_backup_policy),
        )
        .route("/api/v1/backup-policies/prune", post(prune_backup_policies))
        .route(
            "/api/v1/backups/{backup_request_id}/artifact-metadata",
            post(record_backup_artifact_metadata),
        )
        .route(
            "/api/v1/backups/{backup_request_id}/artifact-handoff",
            post(create_backup_artifact_handoff),
        )
        .route(
            "/api/v1/backups/{backup_request_id}/artifact-upload-sessions",
            post(create_backup_artifact_upload_session),
        )
        .route(
            "/api/v1/backups/{backup_request_id}/artifact-upload-sessions/{upload_id}/chunks",
            post(upload_backup_artifact_session_chunk),
        )
        .route(
            "/api/v1/backups/{backup_request_id}/artifact-upload-sessions/{upload_id}/commit",
            post(commit_backup_artifact_upload_session),
        )
        .route(
            "/api/v1/backups/{backup_request_id}/artifact-upload-sessions/{upload_id}/abort",
            post(abort_backup_artifact_upload_session),
        )
        .route(
            "/api/v1/backups/{backup_request_id}/artifact",
            get(download_backup_artifact)
                .post(upload_backup_artifact)
                .layer(DefaultBodyLimit::max(MAX_BACKUP_ARTIFACT_UPLOAD_BODY_BYTES)),
        )
        .route(
            "/api/v1/backups/{backup_request_id}/artifact/prepare-restore",
            post(prepare_backup_artifact_restore)
                .layer(DefaultBodyLimit::max(MAX_BACKUP_ARTIFACT_UPLOAD_BODY_BYTES)),
        )
        .route(
            "/api/v1/restore-plans",
            get(list_restore_plans).post(create_restore_plan),
        )
        .route(
            "/api/v1/migration-links",
            get(list_migration_links).post(create_migration_link),
        )
        .route("/api/v1/audit", get(list_audit_logs))
        .route(
            "/internal/v1/gateway/agent-identity",
            post(validate_agent_identity),
        )
        .route("/internal/v1/gateway/agent-hello", post(ingest_agent_hello))
        .route(
            "/internal/v1/gateway/session-started",
            post(ingest_gateway_session_started),
        )
        .route(
            "/internal/v1/gateway/session-ended",
            post(ingest_gateway_session_ended),
        )
        .route("/internal/v1/gateway/telemetry", post(ingest_telemetry))
        .route(
            "/internal/v1/gateway/command-output",
            post(ingest_command_output),
        )
        .route(
            "/internal/v1/gateway/terminal-output",
            post(ingest_terminal_output),
        )
        .route("/ws", get(ws_handler))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}
