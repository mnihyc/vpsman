use std::{
    collections::{HashMap, HashSet},
    panic::AssertUnwindSafe,
    sync::OnceLock,
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use futures_util::FutureExt;
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, types::Json as SqlJson, PgPool, Postgres, Row, Transaction};
#[cfg(test)]
use tokio::task::JoinError;
use tokio::{
    sync::{watch, Notify},
    task::JoinHandle,
};
use tracing::{debug, warn};
use uuid::Uuid;
use vpsman_common::{
    ordinal_admission_mask_has_exact_shape, projected_telemetry_tunnel_identity,
    structurally_valid_projected_telemetry_tunnel, AgentMetrics, AgentUpdateHeartbeat,
    GatewayAgentHelloIngest, GatewayTelemetryIngest, JobCommand, NetworkInterfacePolicy,
    NetworkInterfaceSource, ProjectedTelemetryTunnelIdentity,
    DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS,
};
use vpsman_server_core::{TARGET_STATUS_AGENT_LOST, TARGET_STATUS_COMPLETED, TARGET_STATUS_FAILED};

use crate::model_alert_policies::{AlertPolicyRuleKind, TrafficAccountingRecord};
use crate::repository::Repository;
use crate::repository_agent_update_lifecycle::record_agent_update_heartbeat_in_tx;
use crate::repository_alert_policies::{
    advance_projected_traffic_accounting_frontier, load_projected_traffic_accounting_context_in_tx,
    rebase_projected_traffic_accounting_frontier_in_tx,
    refresh_projected_traffic_accounting_durable_streams_in_tx, ProjectedTrafficAccountingContext,
    ProjectedTrafficAccountingFrontier, ProjectedTrafficCounter, ProjectedTrafficCounterOverlay,
    TrafficStreamIdentity,
};
use crate::repository_jobs::{
    append_synthetic_agent_lost_output_in_tx, append_synthetic_status_output_in_tx,
    enqueue_target_terminal_event_in_tx, finish_jobs_in_tx_and_reconcile_event_sources,
};
use crate::repository_key_lifecycle::public_key_sha256_hex;
use crate::repository_monitoring::accepted_postgres_ping_results;
use crate::repository_network_observations::{
    record_postgres_automatic_tunnel_reachability_suffix_in_tx, AutomaticTunnelReachabilitySample,
    FrozenAutomaticTunnelPlan,
};
use crate::repository_operational_alerts::{
    mark_postgres_tunnel_alerts_unknown_for_clients_in_tx,
    reconcile_postgres_agent_alert_transition_in_tx,
    reconcile_postgres_tunnel_alerts_for_clients_in_tx,
};
use crate::repository_policy_lifecycle::{
    load_policy_evidence_rule_set_in_tx, materialize_policy_evidence_baseline_in_tx,
    record_policy_evidence_with_rule_set_in_tx, PolicyEvidenceFact, PolicyEvidenceRuleSet,
};
use crate::repository_port_forwarding::record_postgres_port_forward_runtime_snapshot_in_tx;
use crate::repository_telemetry_policy_activation::{
    claim_telemetry_policy_activation_generation_in_tx,
    enqueue_telemetry_policy_activation_sample_in_tx, wake_telemetry_policy_activation,
    wake_telemetry_policy_activation_after_projection,
};
use crate::repository_terminal_sessions::mark_postgres_terminal_session_agent_lost_in_tx;
use crate::security::constant_time_eq;
const TELEMETRY_PROJECTOR_IDLE_POLL: Duration = Duration::from_secs(1);
const CURRENT_PING_LOSS_WINDOW_SECS: i64 = 15 * 60;

static TELEMETRY_PROJECTOR_WAKE: OnceLock<Notify> = OnceLock::new();

fn telemetry_projector_wake() -> &'static Notify {
    TELEMETRY_PROJECTOR_WAKE.get_or_init(Notify::new)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TelemetryRecordOutcome {
    Recorded,
    AcceptedDuplicate,
    AcceptedStale,
    GatewaySessionNotActive,
}

fn terminal_outcome(
    status: &str,
    message: impl Into<String>,
    exit_code: Option<i32>,
    accepted: bool,
) -> crate::TargetDispatchOutcome {
    crate::TargetDispatchOutcome {
        status: status.to_string(),
        exit_code,
        #[cfg(test)]
        command_version: None,
        accepted,
        message: message.into(),
        received_at: None,
        outputs: Vec::new(),
    }
}

async fn mark_old_incarnation_targets_agent_lost_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    previous_process_incarnation_id: Uuid,
    current_process_incarnation_id: Uuid,
    gateway_id: &str,
    update_heartbeat: Option<&AgentUpdateHeartbeat>,
) -> Result<Vec<Uuid>> {
    let rows = sqlx::query(
        r#"
        SELECT target.job_id, target.client_id, job.operation, job.payload_hash
        FROM job_targets target
        JOIN jobs job ON job.id = target.job_id
        WHERE target.client_id = $1
          AND target.completed_at IS NULL
          AND target.status IN ('dispatching', 'running')
          AND target.process_incarnation_id = $2
        ORDER BY target.job_id, target.client_id
        FOR UPDATE
        "#,
    )
    .bind(client_id)
    .bind(previous_process_incarnation_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut affected_job_ids = Vec::new();
    for row in rows {
        let job_id: Uuid = row.try_get("job_id")?;
        let target_client_id: String = row.try_get("client_id")?;
        let job_payload_hash: String = row.try_get("payload_hash")?;
        let operation = row
            .try_get::<sqlx::types::Json<JobCommand>, _>("operation")
            .map(|operation| operation.0);
        let operation_decode_failed = operation.is_err();
        if let (
            Some(JobCommand::AgentUpdateActivate {
                staged_sha256_hex, ..
            }),
            Some(heartbeat),
        ) = (operation.as_ref().ok(), update_heartbeat)
        {
            if heartbeat.activation_job_id == job_id {
                let expected_sha256_hex = staged_sha256_hex.to_ascii_lowercase();
                let observed_sha256_hex = heartbeat.sha256_hex.to_ascii_lowercase();
                if observed_sha256_hex != expected_sha256_hex {
                    let message = format!(
                        "agent update activation heartbeat reported artifact hash {observed_sha256_hex}, expected {expected_sha256_hex}"
                    );
                    append_synthetic_status_output_in_tx(
                        tx,
                        job_id,
                        &target_client_id,
                        serde_json::json!({
                            "type": "agent_update_activation_heartbeat",
                            "status": TARGET_STATUS_FAILED,
                            "code": "agent_update_activation_heartbeat_hash_mismatch",
                            "message": message,
                            "job_id": job_id,
                            "client_id": &target_client_id,
                            "activation_job_id": heartbeat.activation_job_id,
                            "artifact_sha256_hex": &observed_sha256_hex,
                            "staged_sha256_hex": &expected_sha256_hex,
                            "marker_unix": heartbeat.marker_unix,
                            "observed_unix": heartbeat.observed_unix,
                            "previous_process_incarnation_id": previous_process_incarnation_id,
                            "process_incarnation_id": current_process_incarnation_id,
                        }),
                        Some(1),
                    )
                    .await?;
                    let updated = sqlx::query(
                        r#"
                        UPDATE job_targets
                        SET status = 'failed',
                            message = $3,
                            exit_code = 1,
                            completed_at = now(),
                            result_received_at = to_timestamp($5),
                            dispatch_lease_until = NULL,
                            last_dispatch_error = $3
                        WHERE job_id = $1
                          AND client_id = $2
                          AND completed_at IS NULL
                          AND status IN ('dispatching', 'running')
                          AND process_incarnation_id = $4
                        "#,
                    )
                    .bind(job_id)
                    .bind(&target_client_id)
                    .bind(&message)
                    .bind(previous_process_incarnation_id)
                    .bind(heartbeat.observed_unix as f64)
                    .execute(&mut **tx)
                    .await?;
                    if updated.rows_affected() == 0 {
                        anyhow::bail!(
                            "agent_update_activation_heartbeat_terminal_cas_lost:{job_id}:{target_client_id}"
                        );
                    }
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (
                            id, actor_id, action, target, command_hash, metadata
                        )
                        VALUES ($1, NULL, $2, $3, $4, $5)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind("job.target_result")
                    .bind(format!("client:{target_client_id}"))
                    .bind(&job_payload_hash)
                    .bind(serde_json::json!({
                        "job_id": job_id,
                        "status": TARGET_STATUS_FAILED,
                        "result": TARGET_STATUS_FAILED,
                        "exit_code": 1,
                        "accepted": false,
                        "message": message,
                        "client_id": &target_client_id,
                        "reason": "agent_update_activation_heartbeat_hash_mismatch",
                        "previous_process_incarnation_id": previous_process_incarnation_id,
                        "current_process_incarnation_id": current_process_incarnation_id,
                        "gateway_id": gateway_id,
                        "origin_kind": "gateway_ingest",
                        "component": "agent-update-activation-reconciler",
                    }))
                    .execute(&mut **tx)
                    .await?;
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (
                            id, actor_id, action, target, command_hash, metadata
                        )
                        VALUES ($1, NULL, $2, $3, $4, $5)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind("agent_update.activation_failed")
                    .bind(format!("client:{target_client_id}"))
                    .bind(&job_payload_hash)
                    .bind(serde_json::json!({
                        "activation_job_id": job_id,
                        "client_id": &target_client_id,
                        "artifact_sha256_hex": &expected_sha256_hex,
                        "observed_artifact_sha256_hex": &observed_sha256_hex,
                        "status": "activation_failed",
                        "result": "failed",
                        "reason": "heartbeat_hash_mismatch",
                        "gateway_id": gateway_id,
                        "origin_kind": "gateway_ingest",
                        "component": "agent-update-activation-reconciler",
                    }))
                    .execute(&mut **tx)
                    .await?;
                    let outcome =
                        terminal_outcome(TARGET_STATUS_FAILED, message.clone(), Some(1), false);
                    enqueue_target_terminal_event_in_tx(tx, job_id, &target_client_id, &outcome)
                        .await?;
                    affected_job_ids.push(job_id);
                    continue;
                }
                let message = "agent update activation heartbeat verified after restart";
                append_synthetic_status_output_in_tx(
                    tx,
                    job_id,
                    &target_client_id,
                    serde_json::json!({
                        "type": "agent_update_activation_heartbeat",
                        "status": TARGET_STATUS_COMPLETED,
                        "code": "agent_update_restart_heartbeat_verified",
                        "message": message,
                        "job_id": job_id,
                        "client_id": &target_client_id,
                        "activation_job_id": heartbeat.activation_job_id,
                        "artifact_sha256_hex": &observed_sha256_hex,
                        "staged_sha256_hex": &expected_sha256_hex,
                        "marker_unix": heartbeat.marker_unix,
                        "observed_unix": heartbeat.observed_unix,
                        "previous_process_incarnation_id": previous_process_incarnation_id,
                        "process_incarnation_id": current_process_incarnation_id,
                    }),
                    Some(0),
                )
                .await?;
                let updated = sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = 'completed',
                        message = $3,
                        exit_code = 0,
                        completed_at = now(),
                        result_received_at = to_timestamp($5),
                        dispatch_lease_until = NULL,
                        last_dispatch_error = NULL
                    WHERE job_id = $1
                      AND client_id = $2
                      AND completed_at IS NULL
                      AND status IN ('dispatching', 'running')
                      AND process_incarnation_id = $4
                    "#,
                )
                .bind(job_id)
                .bind(&target_client_id)
                .bind(message)
                .bind(previous_process_incarnation_id)
                .bind(heartbeat.observed_unix as f64)
                .execute(&mut **tx)
                .await?;
                if updated.rows_affected() == 0 {
                    anyhow::bail!(
                        "agent_update_activation_heartbeat_terminal_cas_lost:{job_id}:{target_client_id}"
                    );
                }
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, $4, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("job.target_result")
                .bind(format!("client:{target_client_id}"))
                .bind(&job_payload_hash)
                .bind(serde_json::json!({
                    "job_id": job_id,
                    "status": TARGET_STATUS_COMPLETED,
                    "result": TARGET_STATUS_COMPLETED,
                    "exit_code": 0,
                    "accepted": true,
                    "message": message,
                    "client_id": &target_client_id,
                    "reason": "agent_update_restart_heartbeat_verified",
                    "previous_process_incarnation_id": previous_process_incarnation_id,
                    "current_process_incarnation_id": current_process_incarnation_id,
                    "gateway_id": gateway_id,
                    "origin_kind": "gateway_ingest",
                    "component": "agent-update-activation-reconciler",
                }))
                .execute(&mut **tx)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, $4, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("agent_update.activation_completed")
                .bind(format!("client:{target_client_id}"))
                .bind(&job_payload_hash)
                .bind(serde_json::json!({
                    "activation_job_id": job_id,
                    "client_id": &target_client_id,
                    "artifact_sha256_hex": &expected_sha256_hex,
                    "status": "activation_completed",
                    "result": "succeeded",
                    "heartbeat": "verified_after_restart",
                    "gateway_id": gateway_id,
                    "origin_kind": "gateway_ingest",
                    "component": "agent-update-activation-reconciler",
                }))
                .execute(&mut **tx)
                .await?;
                let outcome = terminal_outcome(TARGET_STATUS_COMPLETED, message, Some(0), true);
                enqueue_target_terminal_event_in_tx(tx, job_id, &target_client_id, &outcome)
                    .await?;
                affected_job_ids.push(job_id);
                continue;
            }
        }
        let (message, reason) = if operation_decode_failed {
            (
                format!(
                    "agent process incarnation changed from {previous_process_incarnation_id} to {current_process_incarnation_id} before final command output; stored job operation is missing or invalid"
                ),
                "agent_process_incarnation_changed_invalid_job_operation",
            )
        } else {
            (
                format!(
                    "agent process incarnation changed from {previous_process_incarnation_id} to {current_process_incarnation_id} before final command output"
                ),
                "agent_process_incarnation_changed",
            )
        };
        append_synthetic_agent_lost_output_in_tx(
            tx,
            job_id,
            &target_client_id,
            &message,
            Some(previous_process_incarnation_id),
            Some(current_process_incarnation_id),
        )
        .await?;
        let updated = sqlx::query(
            r#"
            UPDATE job_targets
            SET status = 'agent_lost',
                message = $3,
                completed_at = now(),
                result_received_at = now(),
                dispatch_lease_until = NULL,
                cancel_requested_at = COALESCE(cancel_requested_at, now()),
                last_dispatch_error = $3
            WHERE job_id = $1
              AND client_id = $2
              AND completed_at IS NULL
              AND status IN ('dispatching', 'running')
              AND process_incarnation_id = $4
            "#,
        )
        .bind(job_id)
        .bind(&target_client_id)
        .bind(&message)
        .bind(previous_process_incarnation_id)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() == 0 {
            continue;
        }
        mark_postgres_terminal_session_agent_lost_in_tx(tx, job_id, &target_client_id).await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_id, action, target, command_hash, metadata
            )
            VALUES ($1, NULL, $2, $3, $4, $5)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind("job.target_result")
        .bind(format!("client:{target_client_id}"))
        .bind(&job_payload_hash)
        .bind(serde_json::json!({
            "job_id": job_id,
            "status": TARGET_STATUS_AGENT_LOST,
            "result": TARGET_STATUS_AGENT_LOST,
            "message": message,
            "reason": reason,
            "operation_decode_failed": operation_decode_failed,
            "gateway_id": gateway_id,
            "previous_process_incarnation_id": previous_process_incarnation_id,
            "current_process_incarnation_id": current_process_incarnation_id,
            "origin_kind": "gateway_ingest",
            "component": "agent-incarnation-reconciler",
        }))
        .execute(&mut **tx)
        .await?;
        let outcome = terminal_outcome(TARGET_STATUS_AGENT_LOST, message.clone(), None, false);
        enqueue_target_terminal_event_in_tx(tx, job_id, &target_client_id, &outcome).await?;
        affected_job_ids.push(job_id);
    }
    affected_job_ids.sort();
    affected_job_ids.dedup();
    finish_jobs_in_tx_and_reconcile_event_sources(tx, &affected_job_ids).await?;
    Ok(affected_job_ids)
}

impl Repository {
    pub(crate) async fn validate_agent_public_key(
        &self,
        client_id: &str,
        noise_public_key_hex: &str,
    ) -> Result<bool> {
        let provided = hex::decode(noise_public_key_hex).with_context(|| {
            format!("invalid noise public key hex for identity validation: {client_id}")
        })?;
        if provided.len() != 32 {
            return Ok(false);
        }
        let provided_fingerprint = public_key_sha256_hex(&provided);
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        public_key,
                        status NOT IN ('revoked', 'deleted')
                            AND NOT EXISTS (
                                SELECT 1
                                FROM client_key_revocations
                                WHERE public_key_sha256_hex = $2
                            ) AS identity_active
                    FROM visible_clients
                    WHERE id = $1
                    "#,
                )
                .bind(client_id)
                .bind(provided_fingerprint)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(false);
                };
                let expected: Vec<u8> = row.try_get("public_key")?;
                let identity_active: bool = row.try_get("identity_active")?;
                Ok(identity_active && constant_time_eq(&expected, &provided))
            }
        }
    }

    pub(crate) async fn upsert_agent_hello(&self, event: &GatewayAgentHelloIngest) -> Result<bool> {
        let update_heartbeat = event.hello.update_heartbeat.clone();
        let accepted_hello;
        let authenticated_public_key =
            hex::decode(&event.noise_public_key_hex).with_context(|| {
                format!("invalid noise public key hex for {}", event.hello.client_id)
            })?;
        if authenticated_public_key.len() != 32 {
            return Ok(false);
        }
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let prior = sqlx::query(
                    r#"
                    SELECT
                        status,
                        public_key,
                        internal_build_number,
                        stale_build_number,
                        process_incarnation_id
                    FROM visible_clients
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(&event.hello.client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(prior_row) = prior.as_ref() else {
                    return Ok(false);
                };
                let current_public_key: Vec<u8> = prior_row.try_get("public_key")?;
                let current_status: String = prior_row.try_get("status")?;
                let revoked = sqlx::query(
                    r#"
                    SELECT 1
                    FROM client_key_revocations
                    WHERE public_key_sha256_hex = $1
                    LIMIT 1
                    "#,
                )
                .bind(public_key_sha256_hex(&authenticated_public_key))
                .fetch_optional(&mut *tx)
                .await?
                .is_some();
                if !constant_time_eq(&current_public_key, &authenticated_public_key)
                    || matches!(current_status.as_str(), "revoked" | "deleted")
                    || revoked
                {
                    return Ok(false);
                }
                let prior_status = prior
                    .as_ref()
                    .and_then(|row| row.try_get::<String, _>("status").ok());
                let prior_build = prior
                    .as_ref()
                    .and_then(|row| row.try_get::<i64, _>("internal_build_number").ok())
                    .unwrap_or(1)
                    .max(1);
                let stale_build = prior
                    .as_ref()
                    .and_then(|row| row.try_get::<Option<i64>, _>("stale_build_number").ok())
                    .flatten()
                    .unwrap_or(prior_build)
                    .max(1);
                let prior_process_incarnation_id = prior
                    .as_ref()
                    .and_then(|row| {
                        row.try_get::<Option<Uuid>, _>("process_incarnation_id")
                            .ok()
                    })
                    .flatten();
                let clears_stale = prior_status.as_deref() == Some("stale")
                    && event.hello.internal_build_number as i64 != stale_build;
                let process_incarnation_changed = prior_process_incarnation_id
                    .is_some_and(|prior| prior != event.hello.process_incarnation_id);
                let prior_session_is_same: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM gateway_sessions
                        WHERE id = $1
                          AND client_id = $2
                          AND status = 'active'
                    )
                    "#,
                )
                .bind(event.gateway_session_id)
                .bind(&event.hello.client_id)
                .fetch_one(&mut *tx)
                .await?;
                let result = sqlx::query(
                    r#"
                    INSERT INTO clients (
                        id, display_name, public_key, status, agent_version,
                        internal_build_number, process_incarnation_id, os_release, arch,
                        cpu_model, kernel_release, virtualization, system_reported_at,
                        capabilities, registration_ip,
                        last_ip, last_seen_at
                    )
                    VALUES (
                        $1, $2, $3, 'online', $4, $5, $6, $7, $8,
                        $9, $10, $11, now(), $12, $13::inet, $13::inet, now()
                    )
                    ON CONFLICT (id) DO UPDATE SET
                        status = CASE
                            WHEN clients.status = 'stale'
                             AND EXCLUDED.internal_build_number = COALESCE(clients.stale_build_number, clients.internal_build_number)
                                THEN 'stale'
                            ELSE 'online'
                        END,
                        agent_version = EXCLUDED.agent_version,
                        internal_build_number = EXCLUDED.internal_build_number,
                        process_incarnation_id = EXCLUDED.process_incarnation_id,
                        os_release = EXCLUDED.os_release,
                        arch = EXCLUDED.arch,
                        cpu_model = EXCLUDED.cpu_model,
                        kernel_release = EXCLUDED.kernel_release,
                        virtualization = EXCLUDED.virtualization,
                        system_reported_at = EXCLUDED.system_reported_at,
                        capabilities = EXCLUDED.capabilities,
                        registration_ip = COALESCE(clients.registration_ip, EXCLUDED.registration_ip),
                        last_ip = COALESCE(EXCLUDED.last_ip, clients.last_ip),
                        last_seen_at = now(),
                        stale_since = CASE
                            WHEN clients.status = 'stale'
                             AND EXCLUDED.internal_build_number = COALESCE(clients.stale_build_number, clients.internal_build_number)
                                THEN clients.stale_since
                            ELSE NULL
                        END,
                        stale_reason = CASE
                            WHEN clients.status = 'stale'
                             AND EXCLUDED.internal_build_number = COALESCE(clients.stale_build_number, clients.internal_build_number)
                                THEN clients.stale_reason
                            ELSE NULL
                        END,
                        stale_build_number = CASE
                            WHEN clients.status = 'stale'
                             AND EXCLUDED.internal_build_number = COALESCE(clients.stale_build_number, clients.internal_build_number)
                                THEN clients.stale_build_number
                            ELSE NULL
                        END,
                        suspended_at = NULL,
                        suspended_by = NULL,
                        suspended_reason = NULL,
                        suspended_from_status = NULL
                    WHERE clients.hidden_at IS NULL
                    "#,
                )
                .bind(&event.hello.client_id)
                .bind(&event.hello.client_id)
                .bind(&authenticated_public_key)
                .bind(&event.hello.agent_version)
                .bind(event.hello.internal_build_number as i64)
                .bind(event.hello.process_incarnation_id)
                .bind(&event.hello.os_release)
                .bind(&event.hello.arch)
                .bind(&event.hello.cpu_model)
                .bind(&event.hello.kernel_release)
                .bind(&event.hello.virtualization)
                .bind(sqlx::types::Json(&event.hello.capabilities))
                .bind(event.remote_ip.as_deref())
                .execute(&mut *tx)
                .await?;
                accepted_hello = result.rows_affected() > 0;
                if accepted_hello && process_incarnation_changed {
                    if let Some(previous_process_incarnation_id) = prior_process_incarnation_id {
                        mark_old_incarnation_targets_agent_lost_in_tx(
                            &mut tx,
                            &event.hello.client_id,
                            previous_process_incarnation_id,
                            event.hello.process_incarnation_id,
                            &event.gateway_id,
                            update_heartbeat.as_ref(),
                        )
                        .await?;
                    }
                }
                if accepted_hello {
                    sqlx::query(
                        r#"
                        UPDATE gateway_sessions
                        SET
                            status = 'expired',
                            last_seen_at = now(),
                            ended_at = COALESCE(ended_at, now()),
                            end_reason = COALESCE(end_reason, 'replaced_by_new_session')
                        WHERE client_id = $1
                          AND id <> $2
                          AND status = 'active'
                        "#,
                    )
                    .bind(&event.hello.client_id)
                    .bind(event.gateway_session_id)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        r#"
                        INSERT INTO gateway_sessions (
                            id, gateway_id, client_id, noise_public_key_hex, remote_ip, status
                        )
                        VALUES ($1, $2, $3, $4, $5::inet, 'active')
                        ON CONFLICT (id) DO UPDATE SET
                            gateway_id = EXCLUDED.gateway_id,
                            client_id = EXCLUDED.client_id,
                            noise_public_key_hex = EXCLUDED.noise_public_key_hex,
                            remote_ip = EXCLUDED.remote_ip,
                            status = 'active',
                            last_seen_at = now(),
                            ended_at = NULL,
                            end_reason = NULL
                        "#,
                    )
                    .bind(event.gateway_session_id)
                    .bind(&event.gateway_id)
                    .bind(&event.hello.client_id)
                    .bind(&event.noise_public_key_hex)
                    .bind(event.remote_ip.as_deref())
                    .execute(&mut *tx)
                    .await?;
                }
                let resulting_status = if prior_status.as_deref() == Some("stale") && !clears_stale
                {
                    "stale"
                } else {
                    "online"
                };
                if accepted_hello && prior_status.as_deref() != Some(resulting_status) {
                    let reason = match prior_status.as_deref() {
                        Some("suspended") => "agent_online_auto_unsuspend",
                        Some("never") => "agent_first_connection",
                        Some("stale") => "agent_reconnected_with_changed_internal_build",
                        _ => "agent_reconnected",
                    };
                    record_client_status_transition_in_tx(
                        &mut tx,
                        &event.hello.client_id,
                        prior_status.as_deref(),
                        resulting_status,
                        reason,
                        serde_json::json!({
                            "old_internal_build_number": prior_build,
                            "stale_build_number": stale_build,
                            "new_internal_build_number": event.hello.internal_build_number,
                            "gateway_id": &event.gateway_id,
                        }),
                        "gateway_ingest",
                        "agent-ingest",
                    )
                    .await?;
                } else if accepted_hello && (!prior_session_is_same || process_incarnation_changed)
                {
                    mark_postgres_tunnel_alerts_unknown_for_clients_in_tx(
                        &mut tx,
                        std::slice::from_ref(&event.hello.client_id),
                    )
                    .await?;
                }

                if accepted_hello {
                    if let Some(heartbeat) = update_heartbeat.as_ref() {
                        debug!(
                            client_id = %event.hello.client_id,
                            activation_job_id = %heartbeat.activation_job_id,
                            sha256_hex = %heartbeat.sha256_hex,
                            "recording agent update heartbeat"
                        );
                        record_agent_update_heartbeat_in_tx(
                            &mut tx,
                            &event.hello.client_id,
                            heartbeat,
                        )
                        .await?;
                    }
                }

                tx.commit().await?;
            }
        }
        Ok(accepted_hello)
    }

    pub(crate) async fn record_telemetry_outcome(
        &self,
        event: &GatewayTelemetryIngest,
    ) -> Result<TelemetryRecordOutcome> {
        validate_deferred_telemetry_constraints(&event.telemetry.metrics)?;
        let mut received_metrics = event.telemetry.metrics.clone();
        let reported_observed_unix = received_metrics.observed_unix;
        let ping_source_checked_unix = received_metrics
            .ping_results
            .iter()
            .map(|result| result.checked_unix)
            .collect::<Vec<_>>();
        // Any value accepted into the canonical queue must be projectable;
        // otherwise its contiguous per-client cursor could never advance.
        validated_swap_sample(&received_metrics)?;
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let visible_client = sqlx::query(
                    r#"
                    SELECT client.id, client.status
                    FROM clients client
                    JOIN gateway_sessions session
                      ON session.client_id = client.id
                    WHERE client.id = $1
                      AND client.hidden_at IS NULL
                      AND client.status NOT IN ('revoked', 'suspended')
                      AND client.process_incarnation_id = $2
                      AND session.gateway_id = $3
                      AND session.id = $4
                      AND session.status = 'active'
                    FOR NO KEY UPDATE OF client
                    "#,
                )
                .bind(&event.telemetry.client_id)
                .bind(event.process_incarnation_id)
                .bind(&event.gateway_id)
                .bind(event.gateway_session_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(visible_client) = visible_client else {
                    tx.commit().await?;
                    return Ok(TelemetryRecordOutcome::GatewaySessionNotActive);
                };
                let prior_client_status: String = visible_client.try_get("status")?;
                match claim_postgres_telemetry_sequence(&mut tx, event).await? {
                    TelemetrySequenceClaim::Accepted => {}
                    TelemetrySequenceClaim::Duplicate => {
                        tx.commit().await?;
                        // Replays never bypass the canonical sample's
                        // projector and expose only adapter-owned subsets.
                        return Ok(TelemetryRecordOutcome::AcceptedDuplicate);
                    }
                    TelemetrySequenceClaim::Stale => {
                        tx.commit().await?;
                        return Ok(TelemetryRecordOutcome::AcceptedStale);
                    }
                }
                let telemetry_policy_activation_generation =
                    claim_telemetry_policy_activation_generation_in_tx(&mut tx).await?;
                // This exact head update is the canonical acceptance clock as
                // well as the sequence owner.  The already-held client lock
                // serializes normal routes, while GREATEST also preserves the
                // invariant across a wall-clock step or any future route that
                // reaches this owner directly.  Consecutive samples may share
                // a second and remain ordered by accepted_seq, but their
                // natural UTC minute can never move backward.
                let accepted = sqlx::query(
                    r#"
                    WITH locked_head AS MATERIALIZED (
                        SELECT client_id
                        FROM telemetry_projection_heads
                        WHERE client_id = $1
                        FOR NO KEY UPDATE
                    )
                    UPDATE telemetry_projection_heads AS head
                    SET accepted_seq = head.accepted_seq + 1,
                        accepted_at = GREATEST(
                            head.accepted_at, clock_timestamp()
                        ),
                        projection_retry_at = NULL
                    FROM locked_head
                    WHERE head.client_id = locked_head.client_id
                    RETURNING head.accepted_seq,
                              floor(extract(epoch FROM head.accepted_at))::bigint
                                AS accepted_unix
                    "#,
                )
                .bind(&event.telemetry.client_id)
                .fetch_one(&mut *tx)
                .await?;
                let accepted_seq: i64 = accepted.try_get("accepted_seq")?;
                let received_unix = u64::try_from(accepted.try_get::<i64, _>("accepted_unix")?)
                    .context("canonical telemetry acceptance timestamp is before Unix epoch")?;
                for result in &mut received_metrics.ping_results {
                    let check_age = reported_observed_unix.saturating_sub(result.checked_unix);
                    result.checked_unix = received_unix.saturating_sub(check_age);
                    // The source timestamp is the stable identity of a logical
                    // probe. The rebased timestamp remains the trusted chart
                    // timestamp and follows the canonical acceptance clock.
                }
                for observation in &mut received_metrics.tunnel_reachability {
                    let measurement_age =
                        reported_observed_unix.saturating_sub(observation.measured_unix);
                    observation.measured_unix = received_unix.saturating_sub(measurement_age);
                }
                received_metrics.observed_unix = received_unix;
                let (accepted_ping_results, accepted_ping_source_checked_unix) =
                    accepted_postgres_ping_results(
                        &mut tx,
                        &event.telemetry.client_id,
                        received_metrics.observed_unix,
                        &received_metrics.ping_results,
                        &ping_source_checked_unix,
                    )
                    .await?;
                received_metrics.ping_results = accepted_ping_results;
                let metrics = &received_metrics;
                let sample_id = Uuid::new_v4();
                upsert_postgres_telemetry_sample(
                    &mut tx,
                    sample_id,
                    accepted_seq,
                    event,
                    metrics,
                    &accepted_ping_source_checked_unix,
                )
                .await?;
                let telemetry_policy_activation_queued =
                    if let Some(generation) = telemetry_policy_activation_generation {
                        enqueue_telemetry_policy_activation_sample_in_tx(
                            &mut tx,
                            generation,
                            &event.telemetry.client_id,
                            accepted_seq,
                            sample_id,
                        )
                        .await?
                    } else {
                        false
                    };
                // Liveness is part of acceptance, not the deferred telemetry
                // projection.  The route clears its dispatch fence as soon
                // as this method returns, so heartbeat/status/IP and the
                // corresponding status transition must already be committed.
                let resulting_status: String = sqlx::query_scalar(
                    r#"
                    UPDATE clients
                    SET
                        status = CASE WHEN status = 'stale' THEN status ELSE 'online' END,
                        registration_ip = COALESCE(registration_ip, $2::inet),
                        last_ip = COALESCE($2::inet, last_ip),
                        last_seen_at = now()
                    WHERE id = $1 AND hidden_at IS NULL
                      AND status <> 'revoked'
                    RETURNING status
                    "#,
                )
                .bind(&event.telemetry.client_id)
                .bind(event.remote_ip.as_deref())
                .fetch_one(&mut *tx)
                .await?;
                if telemetry_status_requires_agent_reconciliation(
                    &prior_client_status,
                    &resulting_status,
                ) {
                    reconcile_postgres_agent_alert_transition_in_tx(
                        &mut tx,
                        &event.telemetry.client_id,
                        &resulting_status,
                    )
                    .await?;
                }
                tx.commit().await?;
                // The committed head/raw cursor is the work authority. This
                // process-local signal only avoids an otherwise idle poll;
                // startup and every other API replica discover the same owner
                // directly from PostgreSQL.
                telemetry_projector_wake().notify_one();
                if telemetry_policy_activation_queued {
                    wake_telemetry_policy_activation();
                }
                Ok(TelemetryRecordOutcome::Recorded)
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn record_telemetry(&self, event: &GatewayTelemetryIngest) -> Result<bool> {
        let outcome = self.record_telemetry_outcome(event).await?;
        if outcome == TelemetryRecordOutcome::Recorded {
            // Tests request synchronous projection through this helper.
            // Production callers use `record_telemetry_outcome` and retain the
            // deliberately asynchronous projection boundary.
            self.project_pending_telemetry(1).await?;
        }
        Ok(outcome == TelemetryRecordOutcome::Recorded)
    }

    /// Advances the visible telemetry cursor. One transaction-scoped exact-next
    /// owner captures and atomically projects its complete oldest natural UTC
    /// minute. Every sample still reaches its ordered domain algorithms, while
    /// each bounded owner publishes one final generation. Webhooks consume
    /// their independent cursor from the same canonical samples.
    #[cfg(test)]
    pub(crate) async fn project_pending_telemetry(&self, limit: usize) -> Result<usize> {
        Ok(self.project_pending_telemetry_page(limit).await?.completed)
    }

    #[cfg(test)]
    async fn project_pending_telemetry_page(
        &self,
        limit: usize,
    ) -> Result<TelemetryProjectionPage> {
        let pool = match self {
            Self::Postgres(pool) => pool,
        };
        let (_shutdown_tx, shutdown) = watch::channel(false);
        project_pending_telemetry_page_on_postgres(pool, &shutdown, limit).await
    }
}

#[derive(Debug)]
struct StoredTelemetryProjection {
    id: Uuid,
    accepted_seq: i64,
    accepted_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    metrics: AgentMetrics,
    gateway_session_id: Uuid,
    process_incarnation_id: Uuid,
    telemetry_seq: u64,
    reported_observed_unix: u64,
}

fn telemetry_minute_ready_at_unix(observed_at: DateTime<Utc>) -> Result<i64> {
    observed_at
        .timestamp()
        .div_euclid(60)
        .checked_add(1)
        .and_then(|minute| minute.checked_mul(60))
        .context("telemetry natural-minute deadline is exhausted")
}

/// One sample's immutable admission decision. Plan-current tunnel identity is
/// classified once from plan rows locked by the client projection transaction;
/// every derived network consumer receives this same ordinal decision.
#[derive(Debug)]
pub(crate) struct ProjectedNetworkAdmission {
    network_admitted: Vec<bool>,
    tunnel_admitted: Vec<bool>,
    current_tunnel: Vec<bool>,
}

impl ProjectedNetworkAdmission {
    fn network_mask(&self) -> Vec<u8> {
        pack_ordinal_admission_mask(self.network_admitted.iter().copied())
    }

    fn tunnel_mask(&self) -> Vec<u8> {
        pack_ordinal_admission_mask(self.tunnel_admitted.iter().copied())
    }

    pub(crate) fn network_admitted(&self, ordinal: usize) -> bool {
        self.network_admitted.get(ordinal).copied().unwrap_or(false)
    }

    pub(crate) fn tunnel_admitted(&self, ordinal: usize) -> bool {
        self.tunnel_admitted.get(ordinal).copied().unwrap_or(false)
    }

    fn tunnel_is_current(&self, ordinal: usize) -> bool {
        self.current_tunnel.get(ordinal).copied().unwrap_or(false)
    }
}

fn projected_traffic_counter_overlay(
    observed_at: DateTime<Utc>,
    metrics: &AgentMetrics,
    admission: &ProjectedNetworkAdmission,
) -> ProjectedTrafficCounterOverlay {
    let mut counters = metrics
        .networks
        .iter()
        .enumerate()
        .filter(|(ordinal, _)| admission.network_admitted(*ordinal))
        .map(|(_, network)| ProjectedTrafficCounter {
            source_kind: "host".to_string(),
            interface: network.interface.clone(),
            rx_bytes: u64_to_i64(network.rx_bytes),
            tx_bytes: u64_to_i64(network.tx_bytes),
            sample_source: "agent_networks".to_string(),
        })
        .chain(
            metrics
                .tunnels
                .iter()
                .enumerate()
                .filter(|(ordinal, _)| admission.tunnel_admitted(*ordinal))
                .map(|(_, tunnel)| ProjectedTrafficCounter {
                    source_kind: "tunnel".to_string(),
                    interface: tunnel.interface.clone(),
                    rx_bytes: u64_to_i64(tunnel.rx_bytes),
                    tx_bytes: u64_to_i64(tunnel.tx_bytes),
                    sample_source: tunnel
                        .traffic_source
                        .clone()
                        .unwrap_or_else(|| "runtime_tunnel".to_string()),
                }),
        )
        .collect::<Vec<_>>();
    counters.sort_unstable_by(|left, right| {
        (&left.source_kind, &left.interface).cmp(&(&right.source_kind, &right.interface))
    });
    ProjectedTrafficCounterOverlay {
        observed_at,
        counters,
    }
}

fn projected_traffic_counter_overlay_from_masks(
    metrics: &AgentMetrics,
    network_admission_mask: &[u8],
    tunnel_admission_mask: &[u8],
) -> Result<ProjectedTrafficCounterOverlay> {
    anyhow::ensure!(
        ordinal_admission_mask_has_exact_shape(network_admission_mask, metrics.networks.len())
            && ordinal_admission_mask_has_exact_shape(tunnel_admission_mask, metrics.tunnels.len(),),
        "policy traffic replay has an incomplete admission mask"
    );
    let mask_bit = |mask: &[u8], ordinal: usize| {
        mask.get(ordinal / 8)
            .is_some_and(|byte| byte & (1_u8 << (ordinal % 8)) != 0)
    };
    let observed_at = Utc
        .timestamp_opt(metrics.observed_unix.min(i64::MAX as u64) as i64, 0)
        .single()
        .context("telemetry observed timestamp is invalid")?;
    let mut counters = metrics
        .networks
        .iter()
        .enumerate()
        .filter(|(ordinal, _)| mask_bit(network_admission_mask, *ordinal))
        .map(|(_, network)| ProjectedTrafficCounter {
            source_kind: "host".to_string(),
            interface: network.interface.clone(),
            rx_bytes: u64_to_i64(network.rx_bytes),
            tx_bytes: u64_to_i64(network.tx_bytes),
            sample_source: "agent_networks".to_string(),
        })
        .chain(
            metrics
                .tunnels
                .iter()
                .enumerate()
                .filter(|(ordinal, _)| mask_bit(tunnel_admission_mask, *ordinal))
                .map(|(_, tunnel)| ProjectedTrafficCounter {
                    source_kind: "tunnel".to_string(),
                    interface: tunnel.interface.clone(),
                    rx_bytes: u64_to_i64(tunnel.rx_bytes),
                    tx_bytes: u64_to_i64(tunnel.tx_bytes),
                    sample_source: tunnel
                        .traffic_source
                        .clone()
                        .unwrap_or_else(|| "runtime_tunnel".to_string()),
                }),
        )
        .collect::<Vec<_>>();
    counters.sort_unstable_by(|left, right| {
        (&left.source_kind, &left.interface).cmp(&(&right.source_kind, &right.interface))
    });
    Ok(ProjectedTrafficCounterOverlay {
        observed_at,
        counters,
    })
}

fn projected_traffic_streams(
    overlay: &ProjectedTrafficCounterOverlay,
) -> HashSet<TrafficStreamIdentity> {
    overlay
        .counters
        .iter()
        .map(|counter| (counter.source_kind.clone(), counter.interface.clone()))
        .collect()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TelemetryProjectionPage {
    claimed: usize,
    completed: usize,
    contended: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct TelemetryProjectionClaim {
    client_id: String,
    after_seq: i64,
    through_seq: i64,
    published_generation: i64,
}

enum TelemetryProjectionClaimAttempt {
    Claimed(TelemetryProjectionClaim),
    Contended,
    Idle,
}

#[cfg(test)]
async fn resolve_telemetry_projection_task(
    outcome: std::result::Result<Result<TelemetryProjectionPage>, JoinError>,
) -> Result<TelemetryProjectionPage> {
    match outcome {
        Ok(outcome) => outcome,
        Err(join_error) => {
            // The exact-next immutable raw-journal row is transaction-scoped. A
            // panic or process loss rolls back all derived writes and releases
            // that owner without a durable lease-repair write or expiry.
            warn!(%join_error, "deferred telemetry projection task panicked; transaction rolled back");
            Ok(TelemetryProjectionPage::default())
        }
    }
}

#[cfg(test)]
async fn project_pending_telemetry_page_on_postgres(
    control_pool: &PgPool,
    shutdown: &watch::Receiver<bool>,
    limit: usize,
) -> Result<TelemetryProjectionPage> {
    let mut page = TelemetryProjectionPage::default();
    for _ in 0..limit.max(1) {
        if telemetry_projector_shutdown_requested(shutdown) {
            break;
        }
        let projection_pool = control_pool.clone();
        let projection =
            tokio::spawn(
                async move { project_next_visible_telemetry_suffix(&projection_pool).await },
            );
        let attempt = resolve_telemetry_projection_task(projection.await).await?;
        page.claimed = page.claimed.saturating_add(attempt.claimed);
        page.completed = page.completed.saturating_add(attempt.completed);
        page.contended |= attempt.contended;
        if !telemetry_projection_page_should_continue(attempt) {
            break;
        }
    }
    Ok(page)
}

#[derive(Clone)]
enum TelemetryProjectorRuntime {
    Postgres { control_pool: PgPool },
}

impl TelemetryProjectorRuntime {
    fn new(repo: Repository) -> Self {
        match repo {
            Repository::Postgres(pool) => {
                let control_options = (*pool.connect_options())
                    .clone()
                    .application_name("vpsman-api-telemetry-projector-control");
                let control_pool = PgPoolOptions::new()
                    // One work-conserving owner is the correctness and
                    // performance baseline. It drains every ready client
                    // immediately; no parallel worker may conceal excess
                    // per-client SQL or minute-boundary work.
                    .max_connections(1)
                    .connect_lazy_with(control_options);
                Self::Postgres { control_pool }
            }
        }
    }

    async fn project_page(&self) -> Result<TelemetryProjectionPage> {
        match self {
            Self::Postgres { control_pool } => {
                project_next_visible_telemetry_suffix(control_pool).await
            }
        }
    }

    async fn close(&self) {
        match self {
            Self::Postgres { control_pool } => control_pool.close().await,
        }
    }
}

fn telemetry_projection_page_did_work(page: TelemetryProjectionPage) -> bool {
    page.claimed > 0
}

fn telemetry_projection_page_should_continue(page: TelemetryProjectionPage) -> bool {
    page.claimed > 0 || page.contended
}

#[cfg(test)]
#[test]
fn failed_claimed_projection_page_remains_work_conserving() {
    let failed_owner = TelemetryProjectionPage {
        claimed: 1,
        completed: 0,
        contended: false,
    };
    assert!(telemetry_projection_page_did_work(failed_owner));
    assert!(telemetry_projection_page_should_continue(failed_owner));
    assert!(telemetry_projection_page_should_continue(
        TelemetryProjectionPage {
            contended: true,
            ..TelemetryProjectionPage::default()
        }
    ));
    assert!(!telemetry_projection_page_did_work(
        TelemetryProjectionPage::default()
    ));
    assert!(!telemetry_projection_page_should_continue(
        TelemetryProjectionPage::default()
    ));
}

pub(crate) struct TelemetryProjectorTask {
    shutdown: watch::Sender<bool>,
    handles: Vec<JoinHandle<()>>,
    runtime: TelemetryProjectorRuntime,
}

impl TelemetryProjectorTask {
    pub(crate) fn request_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    pub(crate) async fn wait_for_unexpected_exit(&mut self) -> Result<()> {
        let (result, worker_index, remaining) =
            futures_util::future::select_all(self.handles.iter_mut()).await;
        drop(remaining);
        drop(self.handles.swap_remove(worker_index));
        match result {
            Ok(()) => {
                anyhow::bail!("telemetry projector worker {worker_index} exited unexpectedly")
            }
            Err(error) => Err(error)
                .with_context(|| format!("telemetry projector worker {worker_index} failed")),
        }
    }

    async fn join(mut self) -> Result<()> {
        let mut first_join_error = None;
        for handle in self.handles.drain(..) {
            if let Err(error) = handle.await {
                if first_join_error.is_none() {
                    first_join_error = Some(error);
                }
            }
        }
        self.runtime.close().await;
        match first_join_error {
            Some(error) => Err(error).context("telemetry projector task failed"),
            None => Ok(()),
        }
    }

    pub(crate) async fn shutdown(self) -> Result<()> {
        self.request_shutdown();
        self.join().await
    }
}

fn telemetry_projector_shutdown_requested(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}

async fn telemetry_projector_shutdown_signal(shutdown: &mut watch::Receiver<bool>) {
    while !telemetry_projector_shutdown_requested(shutdown) {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

async fn run_telemetry_projector_worker(
    worker_index: usize,
    runtime: TelemetryProjectorRuntime,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if telemetry_projector_shutdown_requested(&shutdown) {
            break;
        }
        let outcome = AssertUnwindSafe(runtime.project_page())
            .catch_unwind()
            .await;
        let continue_immediately = match outcome {
            Ok(Ok(page)) if telemetry_projection_page_did_work(page) => {
                debug!(
                    worker_index,
                    claimed = page.claimed,
                    completed = page.completed,
                    "projected exact telemetry natural-minute owner"
                );
                true
            }
            Ok(Ok(page)) => telemetry_projection_page_should_continue(page),
            Ok(Err(error)) => {
                warn!(worker_index, %error, "telemetry projector discovery failed");
                false
            }
            Err(_) => {
                warn!(worker_index, "telemetry projector transaction panicked");
                false
            }
        };
        if continue_immediately {
            continue;
        }
        // Notify is only a latency accelerator. A stored permit closes the
        // query-to-wait race, while the bounded poll makes committed work from
        // another process or a crashed predecessor discoverable at startup and
        // across every API replica.
        tokio::select! {
            biased;
            _ = telemetry_projector_shutdown_signal(&mut shutdown) => break,
            _ = telemetry_projector_wake().notified() => {}
            _ = tokio::time::sleep(TELEMETRY_PROJECTOR_IDLE_POLL) => {}
        }
    }
}

pub(crate) fn spawn_telemetry_projector(repo: Repository) -> TelemetryProjectorTask {
    let runtime = TelemetryProjectorRuntime::new(repo);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let handles = vec![tokio::spawn(run_telemetry_projector_worker(
        0,
        runtime.clone(),
        shutdown_rx,
    ))];
    TelemetryProjectorTask {
        shutdown,
        handles,
        runtime,
    }
}

async fn try_claim_telemetry_projection_in_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<TelemetryProjectionClaimAttempt> {
    // The partial visible-pending index has this exact order. The query returns
    // one durable owner instead of materializing the ready fleet, and then the
    // immutable exact-next raw row is the transaction-scoped client lock.
    // SKIP LOCKED lets another API replica continue to a healthy due client;
    // a failed client's retry timestamp therefore cannot hide later owners.
    let candidate = sqlx::query(
        r#"
        SELECT head.client_id, sample.accepted_seq AS owner_seq
        FROM telemetry_projection_heads head
        JOIN telemetry_samples sample
          ON sample.client_id = head.client_id
         AND sample.accepted_seq = head.projected_seq + 1
        WHERE head.projected_seq < head.accepted_seq
          AND COALESCE(head.projection_retry_at, '-infinity'::timestamptz)
                <= clock_timestamp()
        ORDER BY COALESCE(head.projected_at, '-infinity'::timestamptz),
                 head.client_id
        LIMIT 1
        FOR UPDATE OF sample SKIP LOCKED
        "#,
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some(candidate) = candidate else {
        return Ok(TelemetryProjectionClaimAttempt::Idle);
    };
    let client_id: String = candidate.try_get("client_id")?;
    let owner_seq: i64 = candidate.try_get("owner_seq")?;

    // The raw journal row is the immutable client owner and intentionally
    // survives projection through the bounded raw-retention window. A
    // concurrent projector may therefore commit after this statement took its
    // READ COMMITTED snapshot and before it acquired that surviving row. Read
    // the head again in a new statement snapshot after owning the row; only the
    // fresh exact-next row authorizes projection. The head itself remains
    // unlocked so acceptance can keep advancing accepted_seq during projection.
    let Some((projected_seq, through_seq, published_generation)) =
        revalidate_telemetry_projection_owner_in_tx(tx, &client_id, owner_seq).await?
    else {
        return Ok(TelemetryProjectionClaimAttempt::Contended);
    };
    Ok(TelemetryProjectionClaimAttempt::Claimed(
        TelemetryProjectionClaim {
            client_id,
            after_seq: projected_seq,
            through_seq,
            published_generation,
        },
    ))
}

pub(crate) async fn revalidate_telemetry_projection_owner_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    owner_seq: i64,
) -> Result<Option<(i64, i64, i64)>> {
    let fresh = sqlx::query(
        r#"
        SELECT
            head.projected_seq,
            COALESCE((
                SELECT later.accepted_seq - 1
                FROM telemetry_samples later
                WHERE later.client_id = head.client_id
                  AND later.accepted_seq > first_sample.accepted_seq
                  AND later.accepted_seq <= head.accepted_seq
                  AND date_trunc('minute', later.observed_at)
                        IS DISTINCT FROM
                      date_trunc('minute', first_sample.observed_at)
                ORDER BY later.accepted_seq
                LIMIT 1
            ), head.accepted_seq) AS through_seq,
            head.published_generation
        FROM telemetry_projection_heads head
        JOIN telemetry_samples first_sample
          ON first_sample.client_id = head.client_id
         AND first_sample.accepted_seq = head.projected_seq + 1
        WHERE head.client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(fresh) = fresh else {
        return Ok(None);
    };
    let projected_seq: i64 = fresh.try_get("projected_seq")?;
    let exact_next = projected_seq
        .checked_add(1)
        .context("telemetry projection cursor is exhausted")?;
    if owner_seq != exact_next {
        return Ok(None);
    }
    Ok(Some((
        projected_seq,
        fresh.try_get("through_seq")?,
        fresh.try_get("published_generation")?,
    )))
}

async fn load_telemetry_projection_suffix_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    after_seq: i64,
    through_seq: i64,
) -> Result<Vec<StoredTelemetryProjection>> {
    anyhow::ensure!(
        through_seq > after_seq,
        "telemetry projection suffix fence is empty"
    );
    let rows = sqlx::query(
        r#"
        SELECT
            sample.id,
            sample.accepted_seq,
            sample.accepted_at,
            sample.observed_at,
            sample.payload,
            sample.source_gateway_session_id,
            sample.source_process_incarnation_id,
            sample.source_telemetry_seq,
            sample.reported_observed_unix,
            sample.ping_source_checked_unix
        FROM telemetry_samples sample
        WHERE sample.client_id = $1
          AND sample.accepted_seq > $2
          AND sample.accepted_seq <= $3
        ORDER BY sample.accepted_seq
        FOR SHARE OF sample
        "#,
    )
    .bind(client_id)
    .bind(after_seq)
    .bind(through_seq)
    .fetch_all(&mut **tx)
    .await?;
    let expected_len = through_seq
        .checked_sub(after_seq)
        .context("telemetry projection suffix sequence is exhausted")?;
    let actual_len = i64::try_from(rows.len())
        .context("telemetry projection suffix cardinality is exhausted")?;
    anyhow::ensure!(
        actual_len == expected_len,
        "telemetry projection source sequence is not contiguous"
    );
    let mut samples = Vec::with_capacity(rows.len());
    for (offset, row) in rows.into_iter().enumerate() {
        let offset =
            i64::try_from(offset).context("telemetry projection suffix offset is exhausted")?;
        let expected_seq = after_seq
            .checked_add(offset)
            .and_then(|value| value.checked_add(1))
            .context("telemetry projection suffix sequence is exhausted")?;
        let accepted_seq: i64 = row.try_get("accepted_seq")?;
        anyhow::ensure!(
            accepted_seq == expected_seq,
            "telemetry projection source sequence is not contiguous"
        );
        let metrics: SqlJson<AgentMetrics> = row.try_get("payload")?;
        let gateway_session_id: Uuid = row.try_get("source_gateway_session_id")?;
        let process_incarnation_id: Uuid = row.try_get("source_process_incarnation_id")?;
        let telemetry_seq: i64 = row.try_get("source_telemetry_seq")?;
        let reported_observed_unix: i64 = row.try_get("reported_observed_unix")?;
        let ping_source_checked_unix = row
            .try_get::<Vec<i64>, _>("ping_source_checked_unix")?
            .into_iter()
            .map(|value| u64::try_from(value).context("negative ping source timestamp"))
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(
            ping_source_checked_unix.len() == metrics.0.ping_results.len(),
            "telemetry projection ping source cardinality mismatch"
        );
        samples.push(StoredTelemetryProjection {
            id: row.try_get("id")?,
            accepted_seq,
            accepted_at: row.try_get("accepted_at")?,
            observed_at: row.try_get("observed_at")?,
            metrics: metrics.0,
            gateway_session_id,
            process_incarnation_id,
            telemetry_seq: u64::try_from(telemetry_seq)
                .context("negative telemetry projection source sequence")?,
            reported_observed_unix: u64::try_from(reported_observed_unix)
                .context("negative telemetry projection reported observed time")?,
        });
    }
    Ok(samples)
}

/// Publishes the bounded, user-facing Ping current state from the same exact
/// client prefix that advances `telemetry_projection_heads`. Ping series are
/// logical identities needed by both the raw suffix and retained history, so
/// they become visible atomically with that prefix. The natural-minute worker
/// remains the sole owner of facts and rollups.
async fn project_ping_current_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    after_seq: i64,
    through_seq: i64,
) -> Result<()> {
    anyhow::ensure!(
        through_seq > after_seq,
        "Ping current projection suffix is empty"
    );

    sqlx::query(
        r#"
        WITH expanded AS MATERIALIZED (
            SELECT
                sample.client_id,
                (ping.value ->> 'target_id')::UUID AS target_id,
                (ping.value ->> 'generation')::BIGINT AS generation
            FROM telemetry_samples sample
            CROSS JOIN LATERAL jsonb_array_elements(
                CASE WHEN jsonb_typeof(sample.payload -> 'ping_results') = 'array'
                    THEN sample.payload -> 'ping_results' ELSE '[]'::JSONB END
            ) ping(value)
            WHERE sample.client_id = $1
              AND sample.accepted_seq > $2
              AND sample.accepted_seq <= $3
        )
        INSERT INTO telemetry_ping_series (client_id, target_id, generation)
        SELECT DISTINCT expanded.client_id,
               expanded.target_id,
               expanded.generation
        FROM expanded
        JOIN ping_targets target ON target.id = expanded.target_id
        ORDER BY expanded.client_id, expanded.target_id, expanded.generation
        ON CONFLICT (client_id, target_id, generation) DO NOTHING
        "#,
    )
    .bind(client_id)
    .bind(after_seq)
    .bind(through_seq)
    .execute(&mut **tx)
    .await?;

    // Retained facts cover the already-closed prefix and the raw suffix covers
    // every projected sample after the independent core-minute cursor, plus
    // the currently claimed prefix through `through_seq`. Canonicalizing on
    // the stable source check identity makes a raw correction shadow its
    // retained predecessor exactly as `telemetry_ping_points` does. The only
    // retained range read is the indexed 900-second window of a touched series;
    // no fleet or historical range is scanned.
    sqlx::query(
        r#"
        WITH core_frontier AS MATERIALIZED (
            SELECT minute.materialized_seq
            FROM telemetry_minute_materialization_heads minute
            WHERE minute.client_id = $1
        ), expanded AS NOT MATERIALIZED (
            SELECT
                sample.id AS evidence_id,
                sample.client_id,
                sample.accepted_seq,
                sample.accepted_at,
                ping.ordinality,
                sample.ping_source_checked_unix[ping.ordinality]
                    AS source_checked_unix,
                (ping.value ->> 'target_id')::UUID AS target_id,
                (ping.value ->> 'generation')::BIGINT AS generation,
                (ping.value ->> 'checked_unix')::BIGINT AS checked_unix,
                ping.value ->> 'status' AS status,
                (ping.value ->> 'latency_avg_ms')::DOUBLE PRECISION
                    AS latency_avg_ms,
                (ping.value ->> 'loss_ratio')::DOUBLE PRECISION AS loss_ratio,
                ping.value ->> 'reason' AS reason
            FROM core_frontier
            JOIN telemetry_samples sample
              ON sample.client_id = $1
             AND sample.accepted_seq > core_frontier.materialized_seq
             AND sample.accepted_seq <= $3
            CROSS JOIN LATERAL jsonb_array_elements(
                CASE WHEN jsonb_typeof(sample.payload -> 'ping_results') = 'array'
                    THEN sample.payload -> 'ping_results' ELSE '[]'::JSONB END
            ) WITH ORDINALITY ping(value, ordinality)
            WHERE ping.ordinality <= cardinality(
                sample.ping_source_checked_unix
            )
        ), raw_evidence AS MATERIALIZED (
            SELECT series.id AS series_id, expanded.*
            FROM expanded
            JOIN ping_targets target
              ON target.id = expanded.target_id
             AND target.enabled
             AND target.generation = expanded.generation
            JOIN ping_target_assignments assignment
              ON assignment.target_id = expanded.target_id
             AND assignment.client_id = expanded.client_id
            JOIN telemetry_ping_series series
              ON series.client_id = expanded.client_id
             AND series.target_id = expanded.target_id
             AND series.generation = expanded.generation
            WHERE expanded.source_checked_unix > 0
              AND expanded.checked_unix > 0
        ), claimed_canonical AS MATERIALIZED (
            SELECT DISTINCT ON (series_id, source_checked_unix)
                   raw.*
            FROM raw_evidence raw
            WHERE raw.accepted_seq > $2
            ORDER BY series_id, source_checked_unix,
                     accepted_seq DESC, ordinality DESC
        ), predecessor_candidates AS MATERIALIZED (
            SELECT
                fact.series_id,
                fact.source_checked_unix,
                fact.checked_unix,
                fact.status,
                fact.latency_avg_ms,
                fact.loss_ratio,
                fact.reason,
                0::INTEGER AS source_priority,
                0::BIGINT AS accepted_seq,
                0::BIGINT AS ordinality
            FROM claimed_canonical claimed
            JOIN telemetry_ping_facts fact
              ON fact.series_id = claimed.series_id
             AND fact.source_checked_unix = claimed.source_checked_unix
            UNION ALL
            SELECT
                raw.series_id,
                raw.source_checked_unix,
                raw.checked_unix,
                raw.status,
                raw.latency_avg_ms,
                raw.loss_ratio,
                raw.reason,
                1::INTEGER AS source_priority,
                raw.accepted_seq,
                raw.ordinality
            FROM claimed_canonical claimed
            JOIN raw_evidence raw
              ON raw.series_id = claimed.series_id
             AND raw.source_checked_unix = claimed.source_checked_unix
             AND raw.accepted_seq <= $2
        ), predecessor AS MATERIALIZED (
            SELECT DISTINCT ON (series_id, source_checked_unix) *
            FROM predecessor_candidates
            ORDER BY series_id, source_checked_unix,
                     source_priority DESC, accepted_seq DESC, ordinality DESC
        ), touched AS MATERIALIZED (
            SELECT DISTINCT claimed.series_id
            FROM claimed_canonical claimed
            LEFT JOIN predecessor prior
              ON prior.series_id = claimed.series_id
             AND prior.source_checked_unix = claimed.source_checked_unix
            WHERE prior.series_id IS NULL
               OR ROW(
                    prior.checked_unix, prior.status, prior.latency_avg_ms,
                    prior.loss_ratio, prior.reason
                  ) IS DISTINCT FROM ROW(
                    claimed.checked_unix, claimed.status,
                    claimed.latency_avg_ms, claimed.loss_ratio, claimed.reason
                  )
        ), window_bounds AS MATERIALIZED (
            SELECT
                touched.series_id,
                GREATEST(
                    COALESCE(
                        extract(epoch FROM current.latest_checked_at)::BIGINT,
                        0
                    ),
                    COALESCE(max(raw.checked_unix), 0)
                ) AS latest_checked_unix
            FROM touched
            LEFT JOIN telemetry_ping_current current
              ON current.series_id = touched.series_id
            LEFT JOIN raw_evidence raw
              ON raw.series_id = touched.series_id
            GROUP BY touched.series_id, current.latest_checked_at
        ), evidence AS MATERIALIZED (
            SELECT
                fact.series_id,
                fact.evidence_id,
                0::BIGINT AS accepted_seq,
                fact.observed_at AS accepted_at,
                0::BIGINT AS ordinality,
                fact.source_checked_unix,
                fact.checked_unix,
                fact.status,
                fact.latency_avg_ms,
                fact.loss_ratio,
                fact.reason,
                0::INTEGER AS source_priority
            FROM window_bounds bounds
            JOIN telemetry_ping_facts fact
              ON fact.series_id = bounds.series_id
             AND fact.checked_unix
                    >= bounds.latest_checked_unix - ($4::BIGINT - 1)
             AND fact.checked_unix <= bounds.latest_checked_unix
            UNION ALL
            SELECT
                raw.series_id,
                raw.evidence_id,
                raw.accepted_seq,
                raw.accepted_at,
                raw.ordinality,
                raw.source_checked_unix,
                raw.checked_unix,
                raw.status,
                raw.latency_avg_ms,
                raw.loss_ratio,
                raw.reason,
                1::INTEGER AS source_priority
            FROM raw_evidence raw
            JOIN touched USING (series_id)
        ), canonical AS MATERIALIZED (
            SELECT DISTINCT ON (series_id, source_checked_unix) *
            FROM evidence
            ORDER BY series_id, source_checked_unix,
                     source_priority DESC, accepted_seq DESC, ordinality DESC
        ), effective_bounds AS MATERIALIZED (
            SELECT series_id, max(checked_unix) AS latest_checked_unix
            FROM canonical
            GROUP BY series_id
        ), windowed AS MATERIALIZED (
            SELECT canonical.*
            FROM canonical
            JOIN effective_bounds USING (series_id)
            WHERE canonical.checked_unix
                    >= effective_bounds.latest_checked_unix - ($4::BIGINT - 1)
              AND canonical.checked_unix <= effective_bounds.latest_checked_unix
        ), summarized AS (
            SELECT
                series_id,
                (array_agg(status ORDER BY checked_unix DESC,
                    source_checked_unix DESC, source_priority DESC,
                    accepted_seq DESC, ordinality DESC))[1] AS latest_status,
                (array_agg(latency_avg_ms ORDER BY checked_unix DESC,
                    source_checked_unix DESC, source_priority DESC,
                    accepted_seq DESC, ordinality DESC))[1] AS latency_avg_ms,
                avg(loss_ratio)::DOUBLE PRECISION AS rolling_loss_ratio,
                (array_agg(left(reason, 512) ORDER BY checked_unix DESC,
                    source_checked_unix DESC, source_priority DESC,
                    accepted_seq DESC, ordinality DESC))[1] AS latest_reason,
                to_timestamp(max(checked_unix)) AS latest_checked_at
            FROM windowed
            GROUP BY series_id
        )
        INSERT INTO telemetry_ping_current AS current (
            series_id, latest_status, latency_avg_ms,
            rolling_loss_ratio, latest_reason, latest_checked_at, updated_at
        )
        SELECT series_id, latest_status, latency_avg_ms,
               rolling_loss_ratio, latest_reason, latest_checked_at,
               clock_timestamp()
        FROM summarized
        ON CONFLICT (series_id) DO UPDATE SET
            latest_status = EXCLUDED.latest_status,
            latency_avg_ms = EXCLUDED.latency_avg_ms,
            rolling_loss_ratio = EXCLUDED.rolling_loss_ratio,
            latest_reason = EXCLUDED.latest_reason,
            latest_checked_at = EXCLUDED.latest_checked_at,
            updated_at = clock_timestamp()
        WHERE ROW(
            current.latest_status,
            current.latency_avg_ms,
            current.rolling_loss_ratio,
            current.latest_reason,
            current.latest_checked_at
        ) IS DISTINCT FROM ROW(
            EXCLUDED.latest_status,
            EXCLUDED.latency_avg_ms,
            EXCLUDED.rolling_loss_ratio,
            EXCLUDED.latest_reason,
            EXCLUDED.latest_checked_at
        )
        "#,
    )
    .bind(client_id)
    .bind(after_seq)
    .bind(through_seq)
    .bind(CURRENT_PING_LOSS_WINDOW_SECS)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn projected_ping_target_ids(samples: &[StoredTelemetryProjection]) -> Result<Vec<Uuid>> {
    let mut target_ids = HashSet::new();
    for sample in samples {
        for result in &sample.metrics.ping_results {
            target_ids.insert(
                Uuid::parse_str(&result.target_id)
                    .context("projected Ping target identity is not a UUID")?,
            );
        }
    }
    let mut target_ids = target_ids.into_iter().collect::<Vec<_>>();
    target_ids.sort_unstable();
    Ok(target_ids)
}

async fn lock_projected_ping_targets_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    target_ids: &[Uuid],
) -> Result<()> {
    if target_ids.is_empty() {
        return Ok(());
    }
    let locked_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT target.id
        FROM ping_targets target
        WHERE target.id = ANY($1::UUID[])
        ORDER BY target.id
        FOR SHARE OF target
        "#,
    )
    .bind(target_ids)
    .fetch_all(&mut **tx)
    .await?;
    debug_assert!(locked_ids.windows(2).all(|pair| pair[0] < pair[1]));
    Ok(())
}

async fn policy_traffic_minute_cursor_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
) -> Result<i64> {
    sqlx::query_scalar(
        r#"
        SELECT materialized_seq
        FROM traffic_counter_minute_heads
        WHERE client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

async fn load_policy_traffic_frontier_state_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    projected_seq: i64,
) -> Result<(i64, Option<ProjectedTrafficAccountingFrontier>)> {
    let row = sqlx::query(
        r#"
        SELECT policy_traffic_materialized_seq, policy_traffic_frontier
        FROM telemetry_projection_heads
        WHERE client_id = $1
          AND projected_seq = $2
        "#,
    )
    .bind(client_id)
    .bind(projected_seq)
    .fetch_one(&mut **tx)
    .await
    .context("active policy traffic frontier lost its projection owner")?;
    Ok((
        row.try_get("policy_traffic_materialized_seq")?,
        row.try_get::<Option<SqlJson<ProjectedTrafficAccountingFrontier>>, _>(
            "policy_traffic_frontier",
        )?
        .map(|frontier| frontier.0),
    ))
}

async fn load_policy_traffic_replay_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    after_seq: i64,
    through_seq: i64,
) -> Result<Vec<ProjectedTrafficCounterOverlay>> {
    anyhow::ensure!(
        through_seq >= after_seq,
        "policy traffic replay cursor is inverted"
    );
    if through_seq == after_seq {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT accepted_seq, payload,
               network_admission_mask, tunnel_admission_mask
        FROM telemetry_samples
        WHERE client_id = $1
          AND accepted_seq > $2
          AND accepted_seq <= $3
        ORDER BY accepted_seq
        FOR SHARE
        "#,
    )
    .bind(client_id)
    .bind(after_seq)
    .bind(through_seq)
    .fetch_all(&mut **tx)
    .await?;
    let expected = usize::try_from(through_seq - after_seq)
        .context("policy traffic replay cardinality is exhausted")?;
    anyhow::ensure!(
        rows.len() == expected,
        "policy traffic replay source sequence is not contiguous"
    );
    rows.into_iter()
        .enumerate()
        .map(|(offset, row)| {
            let offset =
                i64::try_from(offset).context("policy traffic replay offset is exhausted")?;
            anyhow::ensure!(
                row.try_get::<i64, _>("accepted_seq")?
                    == after_seq
                        .checked_add(offset)
                        .and_then(|seq| seq.checked_add(1))
                        .context("policy traffic replay sequence is exhausted")?,
                "policy traffic replay source sequence is not contiguous"
            );
            let metrics = row.try_get::<SqlJson<AgentMetrics>, _>("payload")?.0;
            projected_traffic_counter_overlay_from_masks(
                &metrics,
                &row.try_get::<Vec<u8>, _>("network_admission_mask")?,
                &row.try_get::<Vec<u8>, _>("tunnel_admission_mask")?,
            )
        })
        .collect()
}

async fn rebase_policy_traffic_frontier_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    context: &mut ProjectedTrafficAccountingContext,
    client_id: &str,
    through_seq: i64,
    as_of: DateTime<Utc>,
    metrics: &AgentMetrics,
    projected_streams: &HashSet<TrafficStreamIdentity>,
) -> Result<(
    i64,
    TrafficAccountingRecord,
    ProjectedTrafficAccountingFrontier,
)> {
    // The minute cursor and its normalized stream rows commit atomically but
    // are read at READ COMMITTED statement boundaries.  Retry only if that
    // exact cursor advances across the snapshot/replay reads; no stream lock or
    // fleet-wide owner is introduced.
    loop {
        let materialized_seq = policy_traffic_minute_cursor_in_tx(tx, client_id).await?;
        anyhow::ensure!(
            materialized_seq <= through_seq,
            "policy traffic minute cursor is ahead of projected telemetry"
        );
        refresh_projected_traffic_accounting_durable_streams_in_tx(tx, context).await?;
        let overlays =
            load_policy_traffic_replay_in_tx(tx, client_id, materialized_seq, through_seq).await?;
        let (traffic, frontier) = rebase_projected_traffic_accounting_frontier_in_tx(
            tx,
            context,
            as_of,
            metrics,
            projected_streams,
            &overlays,
        )
        .await?;
        if policy_traffic_minute_cursor_in_tx(tx, client_id).await? == materialized_seq {
            return Ok((materialized_seq, traffic, frontier));
        }
    }
}

async fn project_next_visible_telemetry_suffix(
    pool: &sqlx::PgPool,
) -> Result<TelemetryProjectionPage> {
    let mut tx = pool.begin().await?;
    let claim = match try_claim_telemetry_projection_in_tx(&mut tx).await? {
        TelemetryProjectionClaimAttempt::Claimed(claim) => claim,
        TelemetryProjectionClaimAttempt::Contended => {
            tx.rollback().await?;
            return Ok(TelemetryProjectionPage {
                contended: true,
                ..TelemetryProjectionPage::default()
            });
        }
        TelemetryProjectionClaimAttempt::Idle => {
            tx.rollback().await?;
            return Ok(TelemetryProjectionPage::default());
        }
    };
    sqlx::query("SAVEPOINT telemetry_projection_attempt")
        .execute(&mut *tx)
        .await?;
    match project_claimed_telemetry_suffix_in_tx(&mut tx, &claim).await {
        Ok(()) => {
            sqlx::query("RELEASE SAVEPOINT telemetry_projection_attempt")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            wake_telemetry_policy_activation_after_projection();
            Ok(TelemetryProjectionPage {
                claimed: 1,
                completed: 1,
                contended: false,
            })
        }
        Err(error) => {
            // Roll back every derived row while retaining the exact-next
            // raw-journal owner. The failure marker is therefore ordered against
            // every other projector and additionally CAS-guards the captured
            // projected cursor.
            sqlx::query("ROLLBACK TO SAVEPOINT telemetry_projection_attempt")
                .execute(&mut *tx)
                .await?;
            record_failed_telemetry_projection_in_tx(&mut tx, &claim, &error).await?;
            sqlx::query("RELEASE SAVEPOINT telemetry_projection_attempt")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            warn!(client_id = %claim.client_id, %error, "deferred telemetry projection failed");
            Ok(TelemetryProjectionPage {
                claimed: 1,
                completed: 0,
                contended: false,
            })
        }
    }
}

async fn project_claimed_telemetry_suffix_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    claim: &TelemetryProjectionClaim,
) -> Result<()> {
    let client_id = &claim.client_id;
    let samples =
        load_telemetry_projection_suffix_in_tx(tx, client_id, claim.after_seq, claim.through_seq)
            .await?;
    let latest_sample = samples
        .iter()
        .max_by_key(|sample| (sample.observed_at, sample.accepted_seq))
        .context("telemetry projection suffix is empty")?;
    let retention_minute_ready_at_unix = samples
        .iter()
        .try_fold(None, |earliest, sample| {
            let ready_at = telemetry_minute_ready_at_unix(sample.observed_at)?;
            Ok::<_, anyhow::Error>(Some(
                earliest.map_or(ready_at, |earliest: i64| earliest.min(ready_at)),
            ))
        })?
        .context("telemetry projection suffix has no natural-minute deadline")?;
    let network_interface_policy = load_network_interface_policy_in_tx(tx, client_id).await?;
    let current_tunnel_plans =
        load_current_tunnel_plan_snapshot_in_tx(tx, client_id, &samples).await?;
    let network_admission = samples
        .iter()
        .map(|sample| {
            classify_projected_network_admission(
                &sample.metrics,
                &network_interface_policy,
                &current_tunnel_plans.endpoints,
                &current_tunnel_plans.managed_endpoint_interfaces,
            )
        })
        .collect::<Vec<_>>();
    // Canonical acceptance is already synchronously durable. Everything below,
    // including the projected cursor and NOTIFY, is one replayable client-prefix
    // transaction: a crash keeps the whole suffix or none of it.
    sqlx::query("SET LOCAL synchronous_commit = OFF")
        .execute(&mut **tx)
        .await?;
    let published_generation = claim
        .published_generation
        .checked_add(1)
        .context("telemetry projection generation is exhausted")?;
    sqlx::query("SELECT set_config('vpsman.telemetry_published_generation', $1, TRUE)")
        .bind(published_generation.to_string())
        .execute(&mut **tx)
        .await?;
    // Raw envelopes keep the complete reported interface vector for the
    // bounded detail window. Publish their per-interface admission masks for
    // the complete claimed suffix in one setwise update before advancing the
    // projected cursor.
    update_projected_network_admission_masks_in_tx(tx, &samples, &network_admission).await?;
    // Ping current state is a live projection, not a closed-minute rollup.
    // Exact target SHARE locks serialize this suffix only with topology
    // mutations of the same identities. The following READ COMMITTED
    // statements then see the committed generation/assignment boundary and
    // cannot resurrect current rows after a removal. Suffixes without Ping
    // evidence pay no topology query.
    let ping_target_ids = projected_ping_target_ids(&samples)?;
    if !ping_target_ids.is_empty() {
        lock_projected_ping_targets_in_tx(tx, &ping_target_ids).await?;
        project_ping_current_in_tx(tx, client_id, claim.after_seq, claim.through_seq).await?;
    }
    // Automatic tunnel current state shares the same low-latency projection
    // boundary. Validate and publish the complete suffix setwise; only compact
    // raw locators are written here and the closed-minute consumer owns its
    // historical aggregates.
    let automatic_reachability_samples = samples
        .iter()
        .map(|sample| AutomaticTunnelReachabilitySample {
            sample_id: sample.id,
            accepted_seq: sample.accepted_seq,
            observations: &sample.metrics.tunnel_reachability,
        })
        .collect::<Vec<_>>();
    record_postgres_automatic_tunnel_reachability_suffix_in_tx(
        tx,
        client_id,
        &current_tunnel_plans.automatic_reachability,
        &automatic_reachability_samples,
    )
    .await?;
    // Alert policies consume the already-owned canonical projection. The one
    // rule-load statement also applies the effective activation generation;
    // pending first-enable work therefore adds no evidence/accounting writes.
    let telemetry_policy_rules =
        load_policy_evidence_rule_set_in_tx(tx, "telemetry.combined").await?;
    let mut policy_traffic_context = if telemetry_policy_rules.is_empty() {
        None
    } else {
        Some(
            load_projected_traffic_accounting_context_in_tx(
                tx,
                client_id,
                &current_tunnel_plans.managed_endpoint_interfaces,
            )
            .await?,
        )
    };
    // Disabled policy projection never reads or deserializes stale frontier
    // state.  Its already-required final head update clears that non-business
    // cache, so a later enable rebuilds from the authoritative minute/raw
    // owners.
    let (mut policy_traffic_materialized_seq, mut policy_traffic_frontier) =
        if policy_traffic_context.is_some() {
            load_policy_traffic_frontier_state_in_tx(tx, client_id, claim.after_seq).await?
        } else {
            (0, None)
        };
    if policy_traffic_context.is_some() {
        let current_materialized_seq = policy_traffic_minute_cursor_in_tx(tx, client_id).await?;
        if current_materialized_seq != policy_traffic_materialized_seq {
            policy_traffic_materialized_seq = current_materialized_seq;
            policy_traffic_frontier = None;
        }
    }
    for (sample, admission) in samples.iter().zip(&network_admission) {
        sqlx::query("SELECT set_config('vpsman.telemetry_accepted_at', $1, TRUE)")
            .bind(sample.accepted_at.to_rfc3339())
            .execute(&mut **tx)
            .await?;
        // Resource, Ping, and traffic/network histories are independent
        // natural-minute consumers of this immutable projected journal. The
        // projector publishes their source and never acquires their cursor or
        // derived-row owners.
        let tunnel_alert_revision = upsert_postgres_telemetry_tunnels(
            tx,
            client_id,
            &sample.metrics,
            admission,
            sample.accepted_at,
        )
        .await?;
        if let Some(snapshot) = sample.metrics.port_forwarding.as_ref() {
            let mut snapshot = snapshot.clone();
            snapshot.observed_unix = sample.metrics.observed_unix;
            record_postgres_port_forward_runtime_snapshot_in_tx(tx, client_id, &snapshot).await?;
        }

        // Plan mutations own tunnel alert source entry/exit. A client with no
        // current endpoint has no valid tunnel.adapter/tunnel.traffic identity,
        // so the normal sample path must not repeat an empty reconciliation.
        if tunnel_alert_revision && !current_tunnel_plans.endpoints.is_empty() {
            reconcile_postgres_tunnel_alerts_for_clients_in_tx(
                tx,
                std::slice::from_ref(&client_id.to_string()),
            )
            .await?;
        }

        if !telemetry_policy_rules.is_empty() {
            let observed_at = Utc
                .timestamp_opt(sample.metrics.observed_unix.min(i64::MAX as u64) as i64, 0)
                .single()
                .context("telemetry observed timestamp is invalid")?;
            let traffic_overlay =
                projected_traffic_counter_overlay(observed_at, &sample.metrics, admission);
            let projected_streams = projected_traffic_streams(&traffic_overlay);
            let policy_traffic_context = policy_traffic_context
                .as_mut()
                .context("active telemetry policy has no traffic context")?;
            let advanced = policy_traffic_frontier
                .as_ref()
                .map(|frontier| {
                    advance_projected_traffic_accounting_frontier(
                        &*policy_traffic_context,
                        observed_at,
                        &sample.metrics,
                        &projected_streams,
                        &traffic_overlay,
                        frontier,
                    )
                })
                .transpose()?
                .flatten();
            let (traffic, next_frontier) = if let Some(advanced) = advanced {
                advanced
            } else {
                let (materialized_seq, traffic, frontier) = rebase_policy_traffic_frontier_in_tx(
                    tx,
                    policy_traffic_context,
                    client_id,
                    sample.accepted_seq,
                    observed_at,
                    &sample.metrics,
                    &projected_streams,
                )
                .await?;
                policy_traffic_materialized_seq = materialized_seq;
                (traffic, frontier)
            };
            policy_traffic_frontier = Some(next_frontier);
            record_combined_telemetry_policy_evidence_in_tx(
                tx,
                client_id,
                sample.gateway_session_id,
                sample.process_incarnation_id,
                sample.telemetry_seq,
                sample.id,
                sample.reported_observed_unix,
                &sample.metrics,
                &traffic,
                &telemetry_policy_rules,
            )
            .await?;
        }
    }

    let (committed_generation, sample_prune_ready_at_unix): (i64, Option<i64>) = sqlx::query_as(
        r#"
        WITH prior_current AS MATERIALIZED (
            SELECT
                current_sample.id AS sample_id,
                current_sample.observed_at,
                current_sample.accepted_seq
            FROM telemetry_projection_heads head
            LEFT JOIN telemetry_samples current_sample
              ON current_sample.id = head.latest_projected_sample_id
             AND current_sample.client_id = head.client_id
            WHERE head.client_id = $1
        )
        UPDATE telemetry_projection_heads AS head
        SET projected_seq = $3,
            latest_projected_sample_id = CASE
                WHEN prior_current.sample_id IS NULL
                  OR (prior_current.observed_at, prior_current.accepted_seq)
                        < ($5::timestamptz, $6::bigint)
                THEN $7
                ELSE head.latest_projected_sample_id
            END,
            published_generation = $4,
            projected_at = clock_timestamp(),
            projection_retry_at = NULL,
            projection_attempts = 0,
            projection_error = NULL,
            policy_traffic_materialized_seq = CASE WHEN $8::boolean
                THEN $9 ELSE 0 END,
            policy_traffic_frontier = CASE WHEN $8::boolean
                THEN $10 ELSE NULL::jsonb END
        FROM prior_current
        WHERE head.client_id = $1
          AND head.projected_seq = $2
          AND head.accepted_seq >= $3
        RETURNING
            head.published_generation,
            CASE
                WHEN prior_current.sample_id IS NOT NULL
                 AND head.latest_projected_sample_id
                        IS DISTINCT FROM prior_current.sample_id
                THEN ceil(extract(epoch FROM (
                    prior_current.observed_at
                        + make_interval(days => $11)
                        + interval '1 microsecond'
                )))::bigint
                ELSE NULL::bigint
            END AS sample_prune_ready_at_unix
        "#,
    )
    .bind(client_id)
    .bind(claim.after_seq)
    .bind(claim.through_seq)
    .bind(published_generation)
    .bind(latest_sample.observed_at)
    .bind(latest_sample.accepted_seq)
    .bind(latest_sample.id)
    .bind(policy_traffic_context.is_some())
    .bind(policy_traffic_materialized_seq)
    .bind(
        policy_traffic_frontier
            .as_ref()
            .map(|frontier| SqlJson(frontier.clone())),
    )
    .bind(DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS)
    .fetch_one(&mut **tx)
    .await?;
    let notification = serde_json::json!({
        "client_id": client_id,
        "generation": committed_generation,
        "projected_seq": claim.through_seq,
        "retention_minute_ready_at_unix": retention_minute_ready_at_unix,
        "sample_prune_ready_at_unix": sample_prune_ready_at_unix,
    })
    .to_string();
    sqlx::query("SELECT pg_notify('vpsman_telemetry_projection', $1)")
        .bind(notification)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn load_network_interface_policy_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
) -> Result<NetworkInterfacePolicy> {
    let stored = sqlx::query_scalar::<_, SqlJson<Value>>(
        r#"
        SELECT value_json
        FROM vps_rule_values
        WHERE client_id = $1
          AND key = 'network.interfaces'
        "#,
    )
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    NetworkInterfacePolicy::from_rule_json(stored.as_ref().map(|value| &value.0))
        .map_err(anyhow::Error::msg)
}

#[derive(Debug, Default)]
struct CurrentTunnelPlanSnapshot {
    endpoints: HashSet<ProjectedTelemetryTunnelIdentity>,
    managed_endpoint_interfaces: HashSet<String>,
    automatic_reachability: HashMap<Uuid, FrozenAutomaticTunnelPlan>,
}

fn referenced_tunnel_plan_ids(samples: &[StoredTelemetryProjection]) -> Vec<Uuid> {
    let mut plan_ids = samples
        .iter()
        .flat_map(|sample| {
            sample
                .metrics
                .tunnels
                .iter()
                .filter_map(|tunnel| tunnel.plan_id.as_deref())
                .filter_map(|plan_id| Uuid::parse_str(plan_id.trim()).ok())
                .chain(
                    sample
                        .metrics
                        .tunnel_reachability
                        .iter()
                        .map(|observation| observation.plan_id),
                )
        })
        .collect::<Vec<_>>();
    plan_ids.sort_unstable();
    plan_ids.dedup();
    plan_ids
}

async fn load_current_tunnel_plan_snapshot_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    samples: &[StoredTelemetryProjection],
) -> Result<CurrentTunnelPlanSnapshot> {
    let plan_ids = referenced_tunnel_plan_ids(samples);
    load_current_tunnel_plan_snapshot_for_ids_in_tx(tx, client_id, &plan_ids).await
}

async fn load_current_tunnel_plan_snapshot_for_ids_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    plan_ids: &[Uuid],
) -> Result<CurrentTunnelPlanSnapshot> {
    // The projection client is key-share-locked before any plan. This remains
    // compatible with telemetry acceptance's NO KEY UPDATE liveness write,
    // while plan mutations take endpoint-client UPDATE locks before plan rows.
    // That preserves their lock order instead of forming a client/plan
    // inversion. Plan SHARE blocks
    // every identity-defining update (including non-key fields such as
    // enabled/name/kind/plan/endpoints) until this projection commits. The
    // predicate is the union of every current endpoint owned by this client
    // and the caller's sorted referenced IDs. The former is the authority for
    // absent/default host-interface collision admission; the latter also
    // stabilizes rejected evidence. UUID order gives concurrent multi-plan
    // suffixes one deterministic plan-lock order.
    let rows = sqlx::query(
        r#"
        WITH projection_client AS MATERIALIZED (
            SELECT client.id
            FROM clients client
            WHERE client.id = $2
            FOR KEY SHARE OF client
        ), selected_plan_ids AS MATERIALIZED (
            SELECT requested.plan_id AS id
            FROM projection_client
            CROSS JOIN unnest($1::UUID[]) requested(plan_id)
            UNION
            SELECT plan.id
            FROM projection_client
            JOIN tunnel_plans plan
              ON plan.left_client_id = projection_client.id
            WHERE plan.enabled IS TRUE
              AND plan.deleted_at IS NULL
            UNION
            SELECT plan.id
            FROM projection_client
            JOIN tunnel_plans plan
              ON plan.right_client_id = projection_client.id
            WHERE plan.enabled IS TRUE
              AND plan.deleted_at IS NULL
        )
        SELECT
            plan.id,
            plan.name,
            plan.kind,
            plan.enabled,
            plan.deleted_at IS NULL AS not_deleted,
            plan.plan,
            plan.left_client_id,
            plan.right_client_id
        FROM selected_plan_ids selected
        JOIN tunnel_plans plan ON plan.id = selected.id
        ORDER BY plan.id
        FOR SHARE OF plan
        "#,
    )
    .bind(plan_ids)
    .bind(client_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut snapshot = CurrentTunnelPlanSnapshot {
        endpoints: HashSet::with_capacity(rows.len().saturating_mul(2)),
        managed_endpoint_interfaces: HashSet::with_capacity(rows.len()),
        automatic_reachability: HashMap::with_capacity(rows.len()),
    };
    for row in rows {
        let plan_id: Uuid = row.try_get("id")?;
        let plan_name: String = row.try_get("name")?;
        let kind: String = row.try_get("kind")?;
        let plan = row
            .try_get::<SqlJson<vpsman_common::TunnelPlan>, _>("plan")?
            .0;
        let interface = plan.interface_name.clone();
        let left_client_id: String = row.try_get("left_client_id")?;
        let right_client_id: String = row.try_get("right_client_id")?;
        let current_endpoint = row.try_get::<bool, _>("enabled")?
            && row.try_get::<bool, _>("not_deleted")?
            && (left_client_id == client_id || right_client_id == client_id);
        if !current_endpoint {
            continue;
        }
        snapshot
            .managed_endpoint_interfaces
            .insert(interface.clone());
        if left_client_id == client_id {
            snapshot.endpoints.insert(ProjectedTelemetryTunnelIdentity {
                plan_id,
                plan_name: plan_name.clone(),
                interface: interface.clone(),
                kind: kind.clone(),
                endpoint_side: "left".to_string(),
                peer_client_id: right_client_id.clone(),
            });
        }
        if right_client_id == client_id {
            snapshot.endpoints.insert(ProjectedTelemetryTunnelIdentity {
                plan_id,
                plan_name: plan_name.clone(),
                interface: interface.clone(),
                kind: kind.clone(),
                endpoint_side: "right".to_string(),
                peer_client_id: left_client_id.clone(),
            });
        }
        snapshot.automatic_reachability.insert(
            plan_id,
            FrozenAutomaticTunnelPlan::new(
                plan_id,
                plan_name,
                left_client_id,
                right_client_id,
                plan,
            ),
        );
    }
    Ok(snapshot)
}

#[cfg(test)]
pub(crate) async fn lock_current_tunnel_plan_snapshot_for_test(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    plan_ids: &[Uuid],
) -> Result<usize> {
    let mut plan_ids = plan_ids.to_vec();
    plan_ids.sort_unstable();
    plan_ids.dedup();
    Ok(
        load_current_tunnel_plan_snapshot_for_ids_in_tx(tx, client_id, &plan_ids)
            .await?
            .automatic_reachability
            .len(),
    )
}

fn classify_projected_network_admission(
    metrics: &AgentMetrics,
    policy: &NetworkInterfacePolicy,
    current_plan_endpoints: &HashSet<ProjectedTelemetryTunnelIdentity>,
    managed_endpoint_interfaces: &HashSet<String>,
) -> ProjectedNetworkAdmission {
    let current_tunnel = metrics
        .tunnels
        .iter()
        .map(|tunnel| {
            projected_telemetry_tunnel_identity(tunnel)
                .is_some_and(|identity| current_plan_endpoints.contains(&identity))
        })
        .collect::<Vec<_>>();
    let network_admitted = metrics
        .networks
        .iter()
        .map(|network| {
            valid_telemetry_name(&network.interface)
                && admitted_network_interface(
                    policy,
                    NetworkInterfaceSource::Host,
                    &network.interface,
                    managed_endpoint_interfaces,
                )
        })
        .collect();
    let tunnel_admitted = metrics
        .tunnels
        .iter()
        .zip(&current_tunnel)
        .map(|(tunnel, current)| {
            *current
                && admitted_network_interface(
                    policy,
                    NetworkInterfaceSource::Tunnel,
                    &tunnel.interface,
                    managed_endpoint_interfaces,
                )
        })
        .collect();
    ProjectedNetworkAdmission {
        network_admitted,
        tunnel_admitted,
        current_tunnel,
    }
}

async fn update_projected_network_admission_masks_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    samples: &[StoredTelemetryProjection],
    admission: &[ProjectedNetworkAdmission],
) -> Result<()> {
    anyhow::ensure!(
        samples.len() == admission.len(),
        "telemetry projection admission suffix length mismatch"
    );
    let sample_ids = samples.iter().map(|sample| sample.id).collect::<Vec<_>>();
    let stored_network_admission_masks = admission
        .iter()
        .map(ProjectedNetworkAdmission::network_mask)
        .collect::<Vec<_>>();
    let stored_tunnel_admission_masks = admission
        .iter()
        .map(ProjectedNetworkAdmission::tunnel_mask)
        .collect::<Vec<_>>();
    let updated = sqlx::query(
        r#"
        WITH desired AS MATERIALIZED (
            SELECT sample_id, network_admission_mask, tunnel_admission_mask
            FROM UNNEST(
                $1::UUID[], $2::BYTEA[], $3::BYTEA[]
            ) AS row(
                sample_id, network_admission_mask, tunnel_admission_mask
            )
        )
        UPDATE telemetry_samples sample
        SET network_admission_mask = desired.network_admission_mask,
            tunnel_admission_mask = desired.tunnel_admission_mask
        FROM desired
        WHERE sample.id = desired.sample_id
        "#,
    )
    .bind(&sample_ids)
    .bind(&stored_network_admission_masks)
    .bind(&stored_tunnel_admission_masks)
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        updated.rows_affected() == samples.len() as u64,
        "telemetry projection admission-mask suffix lost a canonical sample"
    );
    Ok(())
}

async fn record_failed_telemetry_projection_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    claim: &TelemetryProjectionClaim,
    error: &anyhow::Error,
) -> Result<()> {
    let updated = sqlx::query(
        r#"
        UPDATE telemetry_projection_heads
        SET projection_attempts = projection_attempts + 1,
            projection_retry_at = clock_timestamp() + interval '1 second',
            projection_error = left($3, 2048)
        WHERE client_id = $1 AND projected_seq = $2
        "#,
    )
    .bind(&claim.client_id)
    .bind(claim.after_seq)
    .bind(format!("{error:#}"))
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        updated.rows_affected() == 1,
        "telemetry projection failure cursor changed outside its client owner"
    );
    Ok(())
}

#[cfg(test)]
mod telemetry_projector_scheduler_tests {
    #[test]
    fn projected_observation_wakes_at_its_exact_next_utc_minute() {
        for (observed_unix, expected_ready_unix) in [(60, 120), (119, 120), (120, 180)] {
            let observed_at = chrono::DateTime::<chrono::Utc>::from_timestamp(observed_unix, 0)
                .expect("test timestamp");
            assert_eq!(
                super::telemetry_minute_ready_at_unix(observed_at).unwrap(),
                expected_ready_unix
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TelemetrySequenceClaim {
    Accepted,
    Duplicate,
    Stale,
}

fn telemetry_status_requires_agent_reconciliation(
    prior_status: &str,
    resulting_status: &str,
) -> bool {
    prior_status != resulting_status
}

#[cfg(test)]
mod telemetry_status_reconciliation_tests {
    use super::telemetry_status_requires_agent_reconciliation;

    #[test]
    fn unchanged_status_skips_agent_reconciliation_but_transition_requires_it() {
        assert!(!telemetry_status_requires_agent_reconciliation(
            "online", "online"
        ));
        assert!(telemetry_status_requires_agent_reconciliation(
            "offline", "online"
        ));
    }
}

async fn claim_postgres_telemetry_sequence(
    tx: &mut Transaction<'_, Postgres>,
    event: &GatewayTelemetryIngest,
) -> Result<TelemetrySequenceClaim> {
    let claimed = sqlx::query_scalar::<_, i32>(
        r#"
        WITH claimed AS (
            INSERT INTO telemetry_ingest_watermarks (
                client_id,
                gateway_session_id,
                process_incarnation_id,
                telemetry_seq,
                reported_observed_unix,
                accepted_at
            )
            VALUES ($1, $2, $3, $4, $5, now())
            ON CONFLICT (client_id) DO UPDATE SET
                gateway_session_id = EXCLUDED.gateway_session_id,
                process_incarnation_id = EXCLUDED.process_incarnation_id,
                telemetry_seq = EXCLUDED.telemetry_seq,
                reported_observed_unix = EXCLUDED.reported_observed_unix,
                accepted_at = now()
            WHERE
                telemetry_ingest_watermarks.gateway_session_id
                    <> EXCLUDED.gateway_session_id
                OR telemetry_ingest_watermarks.process_incarnation_id
                    <> EXCLUDED.process_incarnation_id
                OR telemetry_ingest_watermarks.telemetry_seq < EXCLUDED.telemetry_seq
            RETURNING 1
        )
        SELECT COALESCE((SELECT 1 FROM claimed), 0)
        "#,
    )
    .bind(&event.telemetry.client_id)
    .bind(event.gateway_session_id)
    .bind(event.process_incarnation_id)
    .bind(event.telemetry_seq as i64)
    .bind(event.telemetry.metrics.observed_unix.min(i64::MAX as u64) as i64)
    .fetch_one(&mut **tx)
    .await?;
    if claimed == 1 {
        return Ok(TelemetrySequenceClaim::Accepted);
    }
    let current = sqlx::query_as::<_, (Uuid, Uuid, i64)>(
        r#"
        SELECT gateway_session_id, process_incarnation_id, telemetry_seq
        FROM telemetry_ingest_watermarks
        WHERE client_id = $1
        "#,
    )
    .bind(&event.telemetry.client_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match current {
        Some((session_id, process_id, seq))
            if session_id == event.gateway_session_id
                && process_id == event.process_incarnation_id
                && seq == event.telemetry_seq as i64 =>
        {
            TelemetrySequenceClaim::Duplicate
        }
        _ => TelemetrySequenceClaim::Stale,
    })
}

async fn upsert_postgres_telemetry_sample(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    accepted_seq: i64,
    source_event: &GatewayTelemetryIngest,
    metrics: &AgentMetrics,
    ping_source_checked_unix: &[u64],
) -> Result<()> {
    let disk = persistent_disk_totals(metrics);
    // Acceptance retains the complete bounded raw envelope. The projector
    // stamps its per-interface admission masks before advancing visibility.
    // The saturated BIGINT sentinel keeps an unavailable connection snapshot
    // distinct from an observed zero in the non-null typed columns.
    let tcp_sockets = metrics
        .connections
        .as_ref()
        .map(|connections| u64_to_i64(connections.tcp))
        .unwrap_or(i64::MAX);
    let udp_sockets = metrics
        .connections
        .as_ref()
        .map(|connections| u64_to_i64(connections.udp))
        .unwrap_or(i64::MAX);
    sqlx::query(
        r#"
        INSERT INTO telemetry_samples (
            id,
            client_id,
            observed_at,
            cpu_utilization_ratio,
            cpu_cores,
            cpu_load_1,
            cpu_load_5,
            cpu_load_15,
            memory_total_bytes,
            memory_available_bytes,
            swap_total_bytes,
            swap_available_bytes,
            disk_total_bytes,
            disk_available_bytes,
            tcp_sockets,
            udp_sockets,
            payload,
            accepted_seq,
            accepted_at,
            source_gateway_id,
            source_gateway_session_id,
            source_process_incarnation_id,
            source_telemetry_seq,
            reported_observed_unix,
            ping_source_checked_unix
        ) VALUES (
            $1,
            $2,
            to_timestamp($3::double precision),
            $4,
            $5,
            $6,
            $7,
            $8,
            $9,
            $10,
            $11,
            $12,
            $13,
            $14,
            $15,
            $16,
            $17,
            $18,
            clock_timestamp(),
            $19,
            $20,
            $21,
            $22,
            $23,
            $24
        )
        "#,
    )
    .bind(id)
    .bind(&source_event.telemetry.client_id)
    .bind(metrics.observed_unix as f64)
    .bind(metrics.cpu.utilization_ratio)
    .bind(i32::from(metrics.cpu.cores))
    .bind(metrics.cpu.load.one)
    .bind(metrics.cpu.load.five)
    .bind(metrics.cpu.load.fifteen)
    .bind(u64_to_i64(metrics.memory.total_bytes))
    .bind(u64_to_i64(metrics.memory.available_bytes))
    .bind(metrics.memory.swap_total_bytes.map(u64_to_i64))
    .bind(metrics.memory.swap_available_bytes.map(u64_to_i64))
    .bind(disk.map(|(total, _)| total))
    .bind(disk.map(|(_, available)| available))
    .bind(tcp_sockets)
    .bind(udp_sockets)
    .bind(SqlJson(metrics))
    .bind(accepted_seq)
    .bind(&source_event.gateway_id)
    .bind(source_event.gateway_session_id)
    .bind(source_event.process_incarnation_id)
    .bind(u64_to_i64(source_event.telemetry_seq))
    .bind(u64_to_i64(source_event.telemetry.metrics.observed_unix))
    .bind(
        ping_source_checked_unix
            .iter()
            .copied()
            .map(u64_to_i64)
            .collect::<Vec<_>>(),
    )
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(Into::into)
}

fn validated_swap_sample(metrics: &AgentMetrics) -> Result<Option<(i64, i64)>> {
    match (
        metrics.memory.swap_total_bytes,
        metrics.memory.swap_available_bytes,
    ) {
        (None, None) => Ok(None),
        (Some(total), Some(available)) if available <= total => {
            Ok(Some((u64_to_i64(total), u64_to_i64(available))))
        }
        (Some(_), Some(_)) => anyhow::bail!("swap_available_bytes exceeds swap_total_bytes"),
        _ => anyhow::bail!("swap total and available evidence must be reported together"),
    }
}

async fn upsert_postgres_telemetry_tunnels(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    metrics: &AgentMetrics,
    admission: &ProjectedNetworkAdmission,
    accepted_at: DateTime<Utc>,
) -> Result<bool> {
    let tunnels = metrics
        .tunnels
        .iter()
        .enumerate()
        .filter_map(|(ordinal, tunnel)| {
            admission
                .tunnel_is_current(ordinal)
                .then_some((tunnel, admission.tunnel_admitted(ordinal)))
        })
        .map(|(tunnel, counters_admitted_at_projection)| {
            let telemetry_plan_id = tunnel
                .plan_id
                .as_deref()
                .and_then(|value| Uuid::parse_str(value).ok())
                .context("validated tunnel lost its canonical plan identity")?;
            Ok(serde_json::json!({
                "interface": &tunnel.interface,
                "kind": &tunnel.kind,
                "ownership_mode": &tunnel.ownership_mode,
                "mutation_policy": &tunnel.mutation_policy,
                "source": &tunnel.source,
                "operstate": &tunnel.operstate,
                "mtu": tunnel.mtu.map(u64_to_i64),
                "link_type": tunnel.link_type,
                "address": &tunnel.address,
                "rx_bytes": u64_to_i64(tunnel.rx_bytes),
                "tx_bytes": u64_to_i64(tunnel.tx_bytes),
                "counters_admitted_at_projection": counters_admitted_at_projection,
                "traffic_source": &tunnel.traffic_source,
                "traffic_status": &tunnel.traffic_status,
                "traffic_reason": &tunnel.traffic_reason,
                "traffic_checked_unix": tunnel.traffic_checked_unix.map(u64_to_i64),
                "telemetry_plan_id": telemetry_plan_id,
                "telemetry_topology_identity_hash": &tunnel.topology_identity_hash,
                "telemetry_plan_name": &tunnel.plan_name,
                "telemetry_plan_runtime_manager": &tunnel.plan_runtime_manager,
                "telemetry_endpoint_side": &tunnel.endpoint_side,
                "telemetry_peer_client_id": &tunnel.peer_client_id,
                "adapter_health": tunnel
                    .adapter_health
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()?,
                "latency_monitoring_enabled": tunnel.latency_monitoring_enabled,
                "latency_status": &tunnel.latency_status,
                "latency_reason": &tunnel.latency_reason,
                "latency_primary_family": &tunnel.latency_primary_family,
                "latency_target": &tunnel.latency_target,
                "latency_checked_unix": tunnel.latency_checked_unix.map(u64_to_i64),
                "latency_avg_ms": tunnel.latency_avg_ms,
                "packet_loss_ratio": tunnel.packet_loss_ratio,
                "latency_healthy_windows": tunnel.latency_healthy_windows.map(i32::from),
                "latency_missed_windows": tunnel.latency_missed_windows.map(i32::from),
                "telemetry_runtime_evidence_identity_hash":
                    &tunnel.runtime_evidence_identity_hash,
            }))
        })
        .collect::<Result<Vec<_>>>()?;

    let row = sqlx::query(
        r#"
            WITH incoming AS MATERIALIZED (
                SELECT *
                FROM jsonb_to_recordset($3::JSONB) AS item(
                    interface TEXT,
                    kind TEXT,
                    ownership_mode TEXT,
                    mutation_policy TEXT,
                    source TEXT,
                    operstate TEXT,
                    mtu BIGINT,
                    link_type BIGINT,
                    address TEXT,
                    rx_bytes BIGINT,
                    tx_bytes BIGINT,
                    counters_admitted_at_projection BOOLEAN,
                    traffic_source TEXT,
                    traffic_status TEXT,
                    traffic_reason TEXT,
                    traffic_checked_unix BIGINT,
                    telemetry_plan_id UUID,
                    telemetry_topology_identity_hash TEXT,
                    telemetry_plan_name TEXT,
                    telemetry_plan_runtime_manager TEXT,
                    telemetry_endpoint_side TEXT,
                    telemetry_peer_client_id TEXT,
                    adapter_health JSONB,
                    latency_monitoring_enabled BOOLEAN,
                    latency_status TEXT,
                    latency_reason TEXT,
                    latency_primary_family TEXT,
                    latency_target TEXT,
                    latency_checked_unix BIGINT,
                    latency_avg_ms DOUBLE PRECISION,
                    packet_loss_ratio DOUBLE PRECISION,
                    latency_healthy_windows INTEGER,
                    latency_missed_windows INTEGER,
                    telemetry_runtime_evidence_identity_hash TEXT
                )
            ), alert_revision AS MATERIALIZED (
                SELECT COALESCE(bool_or(
                    stored.interface IS NULL
                    OR ROW(
                        stored.telemetry_plan_id,
                        stored.telemetry_topology_identity_hash,
                        stored.telemetry_runtime_evidence_identity_hash,
                        stored.telemetry_plan_runtime_manager,
                        stored.telemetry_endpoint_side,
                        stored.telemetry_peer_client_id
                    ) IS DISTINCT FROM ROW(
                        incoming.telemetry_plan_id,
                        incoming.telemetry_topology_identity_hash,
                        incoming.telemetry_runtime_evidence_identity_hash,
                        incoming.telemetry_plan_runtime_manager,
                        incoming.telemetry_endpoint_side,
                        incoming.telemetry_peer_client_id
                    )
                    OR ROW(
                        stored.traffic_checked_unix,
                        stored.traffic_status,
                        CASE
                            WHEN stored.traffic_status = 'ok'
                            THEN 'Tunnel interface counters are healthy'
                            WHEN stored.traffic_status IS NULL
                            THEN 'Tunnel traffic counter evidence is unavailable'
                            ELSE COALESCE(
                                stored.traffic_reason,
                                'tunnel interface counters are not reporting ok'
                            )
                        END
                    ) IS DISTINCT FROM ROW(
                        incoming.traffic_checked_unix,
                        incoming.traffic_status,
                        CASE
                            WHEN incoming.traffic_status = 'ok'
                            THEN 'Tunnel interface counters are healthy'
                            WHEN incoming.traffic_status IS NULL
                            THEN 'Tunnel traffic counter evidence is unavailable'
                            ELSE COALESCE(
                                incoming.traffic_reason,
                                'tunnel interface counters are not reporting ok'
                            )
                        END
                    )
                    OR ROW(
                        NULLIF(stored.adapter_health ->> 'checked_unix', '')::BIGINT,
                        (stored.adapter_health ->> 'success')::BOOLEAN,
                        CASE (stored.adapter_health ->> 'success')::BOOLEAN
                            WHEN TRUE THEN 'Tunnel adapter status is healthy'
                            WHEN FALSE THEN COALESCE(
                                stored.adapter_health ->> 'reason',
                                'adapter command did not report healthy status'
                            )
                            ELSE 'Tunnel adapter health evidence is unavailable'
                        END
                    ) IS DISTINCT FROM ROW(
                        NULLIF(incoming.adapter_health ->> 'checked_unix', '')::BIGINT,
                        (incoming.adapter_health ->> 'success')::BOOLEAN,
                        CASE (incoming.adapter_health ->> 'success')::BOOLEAN
                            WHEN TRUE THEN 'Tunnel adapter status is healthy'
                            WHEN FALSE THEN COALESCE(
                                incoming.adapter_health ->> 'reason',
                                'adapter command did not report healthy status'
                            )
                            ELSE 'Tunnel adapter health evidence is unavailable'
                        END
                    )
                    OR (
                        stored.updated_at <= client.operational_alert_tunnel_boundary_at
                        AND $4::TIMESTAMPTZ > client.operational_alert_tunnel_boundary_at
                    )
                    OR (
                        plan.id IS NOT NULL
                        AND stored.updated_at <= plan.operational_alert_runtime_boundary_at
                        AND $4::TIMESTAMPTZ > plan.operational_alert_runtime_boundary_at
                    )
                ), FALSE) AS changed
                FROM incoming
                LEFT JOIN telemetry_tunnels stored
                  ON stored.client_id = $1
                 AND stored.interface = incoming.interface
                JOIN clients client ON client.id = $1
                LEFT JOIN tunnel_plans plan ON plan.id = incoming.telemetry_plan_id
            ), upserted AS (
            INSERT INTO telemetry_tunnels (
                client_id,
                observed_at,
                interface,
                kind,
                ownership_mode,
                mutation_policy,
                source,
                operstate,
                mtu,
                link_type,
                address,
                rx_bytes,
                tx_bytes,
                counters_admitted_at_projection,
                traffic_source,
                traffic_status,
                traffic_reason,
                traffic_checked_unix,
                telemetry_plan_id,
                telemetry_topology_identity_hash,
                telemetry_plan_name,
                telemetry_plan_runtime_manager,
                telemetry_endpoint_side,
                telemetry_peer_client_id,
                adapter_health,
                latency_monitoring_enabled,
                latency_status,
                latency_reason,
                latency_primary_family,
                latency_target,
                latency_checked_unix,
                latency_avg_ms,
                packet_loss_ratio,
                latency_healthy_windows,
                latency_missed_windows,
                telemetry_runtime_evidence_identity_hash,
                updated_at
            )
            SELECT
                $1,
                to_timestamp($2::double precision),
                incoming.interface,
                incoming.kind,
                incoming.ownership_mode,
                incoming.mutation_policy,
                incoming.source,
                incoming.operstate,
                incoming.mtu,
                incoming.link_type,
                incoming.address,
                incoming.rx_bytes,
                incoming.tx_bytes,
                incoming.counters_admitted_at_projection,
                incoming.traffic_source,
                incoming.traffic_status,
                incoming.traffic_reason,
                incoming.traffic_checked_unix,
                incoming.telemetry_plan_id,
                incoming.telemetry_topology_identity_hash,
                incoming.telemetry_plan_name,
                incoming.telemetry_plan_runtime_manager,
                incoming.telemetry_endpoint_side,
                incoming.telemetry_peer_client_id,
                incoming.adapter_health,
                incoming.latency_monitoring_enabled,
                incoming.latency_status,
                incoming.latency_reason,
                incoming.latency_primary_family,
                incoming.latency_target,
                incoming.latency_checked_unix,
                incoming.latency_avg_ms,
                incoming.packet_loss_ratio,
                incoming.latency_healthy_windows,
                incoming.latency_missed_windows,
                incoming.telemetry_runtime_evidence_identity_hash,
                $4::TIMESTAMPTZ
            FROM incoming
            ORDER BY incoming.interface
            ON CONFLICT (client_id, interface) DO UPDATE SET
                observed_at = EXCLUDED.observed_at,
                kind = EXCLUDED.kind,
                ownership_mode = EXCLUDED.ownership_mode,
                mutation_policy = EXCLUDED.mutation_policy,
                source = EXCLUDED.source,
                operstate = EXCLUDED.operstate,
                mtu = EXCLUDED.mtu,
                link_type = EXCLUDED.link_type,
                address = EXCLUDED.address,
                rx_bytes = EXCLUDED.rx_bytes,
                tx_bytes = EXCLUDED.tx_bytes,
                counters_admitted_at_projection =
                    EXCLUDED.counters_admitted_at_projection,
                traffic_source = EXCLUDED.traffic_source,
                traffic_status = EXCLUDED.traffic_status,
                traffic_reason = EXCLUDED.traffic_reason,
                traffic_checked_unix = EXCLUDED.traffic_checked_unix,
                telemetry_plan_id = EXCLUDED.telemetry_plan_id,
                telemetry_topology_identity_hash =
                    EXCLUDED.telemetry_topology_identity_hash,
                telemetry_plan_name = EXCLUDED.telemetry_plan_name,
                telemetry_plan_runtime_manager =
                    EXCLUDED.telemetry_plan_runtime_manager,
                telemetry_endpoint_side = EXCLUDED.telemetry_endpoint_side,
                telemetry_peer_client_id = EXCLUDED.telemetry_peer_client_id,
                adapter_health = EXCLUDED.adapter_health,
                latency_monitoring_enabled = EXCLUDED.latency_monitoring_enabled,
                latency_status = EXCLUDED.latency_status,
                latency_reason = EXCLUDED.latency_reason,
                latency_primary_family = EXCLUDED.latency_primary_family,
                latency_target = EXCLUDED.latency_target,
                latency_checked_unix = EXCLUDED.latency_checked_unix,
                latency_avg_ms = EXCLUDED.latency_avg_ms,
                packet_loss_ratio = EXCLUDED.packet_loss_ratio,
                latency_healthy_windows = EXCLUDED.latency_healthy_windows,
                latency_missed_windows = EXCLUDED.latency_missed_windows,
                telemetry_runtime_evidence_identity_hash =
                    EXCLUDED.telemetry_runtime_evidence_identity_hash,
                updated_at = EXCLUDED.updated_at
            RETURNING interface
            ), deleted AS (
                DELETE FROM telemetry_tunnels stored
                WHERE stored.client_id = $1
                  AND NOT EXISTS (
                      SELECT 1
                      FROM incoming
                      WHERE incoming.interface = stored.interface
                )
                RETURNING stored.interface
            ), reconciled_series AS (
                UPDATE network_observation_series series
                SET active = FALSE
                WHERE series.client_id = $1
                  AND series.active IS TRUE
                  AND NOT EXISTS (
                      SELECT 1
                      FROM incoming
                      WHERE incoming.telemetry_plan_id = series.plan_id
                        AND incoming.interface = series.interface_name
                        AND incoming.telemetry_endpoint_side = series.endpoint_side
                        AND incoming.telemetry_peer_client_id = series.peer_client_id
                        AND incoming.latency_monitoring_enabled IS TRUE
                        AND incoming.latency_primary_family = series.address_family
                        AND incoming.latency_target = series.target
                  )
                RETURNING series.id
            )
            SELECT
                (SELECT count(*) FROM upserted),
                (SELECT count(*) FROM deleted),
                (
                    (SELECT changed FROM alert_revision)
                    OR EXISTS (SELECT 1 FROM deleted)
                ) AS alert_revision
        "#,
    )
    .bind(client_id)
    .bind(metrics.observed_unix as f64)
    .bind(SqlJson(tunnels))
    .bind(accepted_at)
    .fetch_one(&mut **tx)
    .await?;
    row.try_get("alert_revision").map_err(Into::into)
}

fn persistent_disk_totals(metrics: &AgentMetrics) -> Option<(i64, i64)> {
    if !metrics.has_persistent_block_filesystem_disk_sample() {
        return None;
    }
    let disk_total = sum_u64(metrics.disks.iter().map(|disk| disk.total_bytes));
    let disk_available = sum_u64(metrics.disks.iter().map(|disk| disk.available_bytes));
    Some((disk_total, disk_available))
}

/// Packs admission by reported-vector ordinal. PostgreSQL `get_bit(bytea, n)`
/// numbers bits least-significant-first within each byte, so ordinal `n` maps
/// to byte `n / 8`, bit `n % 8`.
fn pack_ordinal_admission_mask(admission: impl ExactSizeIterator<Item = bool>) -> Vec<u8> {
    let mut mask = vec![0_u8; admission.len().div_ceil(8)];
    for (ordinal, admitted) in admission.enumerate() {
        if admitted {
            mask[ordinal / 8] |= 1_u8 << (ordinal % 8);
        }
    }
    mask
}

#[cfg(test)]
fn network_admission_masks(
    metrics: &AgentMetrics,
    policy: &NetworkInterfacePolicy,
    current_plan_endpoints: &HashSet<ProjectedTelemetryTunnelIdentity>,
    managed_endpoint_interfaces: &HashSet<String>,
) -> (Vec<u8>, Vec<u8>) {
    let admission = classify_projected_network_admission(
        metrics,
        policy,
        current_plan_endpoints,
        managed_endpoint_interfaces,
    );
    (admission.network_mask(), admission.tunnel_mask())
}

#[cfg(test)]
mod network_admission_mask_tests {
    use super::{network_admission_masks, pack_ordinal_admission_mask};
    use std::collections::HashSet;
    use vpsman_common::{
        projected_telemetry_tunnel_identity, AgentMetrics, NetworkInterfacePolicy, NetworkStat,
        RuntimeTunnelStat,
    };

    fn valid_tunnel(interface: &str) -> RuntimeTunnelStat {
        RuntimeTunnelStat {
            interface: interface.to_string(),
            kind: "wireguard".to_string(),
            ownership_mode: "managed".to_string(),
            mutation_policy: "managed".to_string(),
            source: "runtime".to_string(),
            plan_id: Some("00000000-0000-0000-0000-000000000001".to_string()),
            plan_name: Some("mask-test".to_string()),
            endpoint_side: Some("left".to_string()),
            peer_client_id: Some("mask-test-peer".to_string()),
            ..RuntimeTunnelStat::default()
        }
    }

    #[test]
    fn ordinal_masks_are_lsb_first_and_cover_partial_final_bytes() {
        let mask = pack_ordinal_admission_mask(
            [
                true, false, false, true, false, false, false, true, false, true,
            ]
            .into_iter(),
        );
        assert_eq!(mask, vec![0x89, 0x02]);
    }

    #[test]
    fn current_tunnel_with_128_byte_plan_name_receives_nonzero_admission_bit() {
        let mut metrics = AgentMetrics {
            tunnels: vec![valid_tunnel("wg0")],
            ..AgentMetrics::default()
        };
        metrics.tunnels[0].plan_name = Some("p".repeat(128));
        let current_plan_endpoints = metrics
            .tunnels
            .iter()
            .filter_map(projected_telemetry_tunnel_identity)
            .collect::<HashSet<_>>();
        let managed_endpoint_interfaces = HashSet::from(["wg0".to_string()]);
        assert_eq!(
            network_admission_masks(
                &metrics,
                &NetworkInterfacePolicy::All,
                &current_plan_endpoints,
                &managed_endpoint_interfaces,
            )
            .1,
            vec![0x01]
        );

        metrics.tunnels[0].plan_name = Some("p".repeat(129));
        assert!(projected_telemetry_tunnel_identity(&metrics.tunnels[0]).is_none());
    }

    #[test]
    fn masks_preserve_policy_validity_and_default_tunnel_collision() {
        let network_names = [
            "eth0", "docker0", "wlan0", "ens3", "veth0", "wg0", "wg4", "w7", "eth8", "br9",
        ];
        let tunnel_names = [
            "wg0", "tun1", "wg2", "tun3", "wg4", "tun5", "tun6", "tun7", "wg8",
        ];
        let mut metrics = AgentMetrics {
            networks: network_names
                .into_iter()
                .map(|interface| NetworkStat {
                    interface: interface.to_string(),
                    rx_bytes: 1,
                    tx_bytes: 1,
                })
                .collect(),
            tunnels: tunnel_names.into_iter().map(valid_tunnel).collect(),
            ..AgentMetrics::default()
        };
        // Invalid projected tunnel evidence remains present in the reported
        // vector and receives no tunnel bit. The current plan still hides its
        // same-named host wg4 under default admission, independent of the
        // malformed runtime row.
        metrics.tunnels[4].plan_id = None;
        let current_plan_endpoints = metrics
            .tunnels
            .iter()
            .filter_map(projected_telemetry_tunnel_identity)
            .collect::<HashSet<_>>();
        let managed_endpoint_interfaces = tunnel_names
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>();

        let default = network_admission_masks(
            &metrics,
            &NetworkInterfacePolicy::DefaultPhysical,
            &current_plan_endpoints,
            &managed_endpoint_interfaces,
        );
        assert_eq!(default.0, vec![0x8d, 0x01]);
        assert_eq!(default.1, vec![0x00, 0x00]);

        let all = network_admission_masks(
            &metrics,
            &NetworkInterfacePolicy::All,
            &current_plan_endpoints,
            &managed_endpoint_interfaces,
        );
        assert_eq!(all.0, vec![0xff, 0x03]);
        assert_eq!(all.1, vec![0xef, 0x01]);

        let patterns = network_admission_masks(
            &metrics,
            &NetworkInterfacePolicy::Patterns(vec!["eth*".to_string(), "wg*".to_string()]),
            &current_plan_endpoints,
            &managed_endpoint_interfaces,
        );
        assert_eq!(patterns.0, vec![0x61, 0x01]);
        assert_eq!(patterns.1, vec![0x05, 0x01]);

        // A malformed or absent runtime row receives no tunnel bit, while the
        // plan-owned interface remains a host collision under the default.
        let stale_endpoints = current_plan_endpoints
            .iter()
            .filter(|identity| identity.interface != "wg0")
            .cloned()
            .collect::<HashSet<_>>();
        let stale_default = network_admission_masks(
            &metrics,
            &NetworkInterfacePolicy::DefaultPhysical,
            &stale_endpoints,
            &managed_endpoint_interfaces,
        );
        assert_eq!(stale_default.0, vec![0x8d, 0x01]);
        assert_eq!(stale_default.1, vec![0x00, 0x00]);
    }
}

/// Applies the single generic network-byte admission policy. Runtime tunnel
/// lifecycle remains independent; only its byte telemetry is gated here.
/// Under the absent/default policy, a host name that is also the identity of a
/// managed runtime tunnel is rejected even when it begins with `w`.
pub(crate) fn admitted_network_interface(
    policy: &NetworkInterfacePolicy,
    source: NetworkInterfaceSource,
    interface: &str,
    current_tunnel_interfaces: &HashSet<String>,
) -> bool {
    if !policy.matches(source, interface) {
        return false;
    }
    if *policy == NetworkInterfacePolicy::DefaultPhysical
        && source == NetworkInterfaceSource::Host
        && current_tunnel_interfaces.contains(interface)
    {
        return false;
    }
    true
}

async fn record_combined_telemetry_policy_evidence_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    gateway_session_id: Uuid,
    process_incarnation_id: Uuid,
    telemetry_seq: u64,
    telemetry_sample_id: Uuid,
    reported_observed_unix: u64,
    metrics: &AgentMetrics,
    traffic: &TrafficAccountingRecord,
    rule_set: &PolicyEvidenceRuleSet,
) -> Result<()> {
    record_policy_evidence_with_rule_set_in_tx(
        tx,
        combined_telemetry_policy_fact(
            client_id,
            gateway_session_id,
            process_incarnation_id,
            telemetry_seq,
            telemetry_sample_id,
            reported_observed_unix,
            metrics,
            traffic,
        )?,
        rule_set,
    )
    .await
    .map(|_| ())
}

fn combined_telemetry_policy_fact(
    client_id: &str,
    gateway_session_id: Uuid,
    process_incarnation_id: Uuid,
    telemetry_seq: u64,
    telemetry_sample_id: Uuid,
    reported_observed_unix: u64,
    metrics: &AgentMetrics,
    traffic: &TrafficAccountingRecord,
) -> Result<PolicyEvidenceFact> {
    let observed_at = Utc
        .timestamp_opt(metrics.observed_unix.min(i64::MAX as u64) as i64, 0)
        .single()
        .context("telemetry observed timestamp is invalid")?;
    Ok(PolicyEvidenceFact {
        source_kind: "telemetry.combined".to_string(),
        source_event_id: format!(
            "telemetry.combined:{gateway_session_id}:{process_incarnation_id}:{telemetry_seq}"
        ),
        fact_kind: AlertPolicyRuleKind::Metric,
        natural_key: client_id.to_string(),
        confirmation_bucket_key: client_id.to_string(),
        subject_client_id: Some(client_id.to_string()),
        target_kind: "client".to_string(),
        target_id: client_id.to_string(),
        source_status: if traffic.state == "ok" {
            "complete".to_string()
        } else {
            "incomplete".to_string()
        },
        // Metric completeness is field-local: absent CPU utilization, quota,
        // or a reset-safe traffic cycle becomes a missing JSON leaf and thus
        // Kleene Unknown only for expressions that reference that fact.
        complete: true,
        // The lifecycle recorder replaces this with the canonical subject
        // snapshot while the telemetry transaction still owns the client.
        subject_snapshot: serde_json::json!({}),
        payload: combined_metric_evidence_payload(
            metrics,
            traffic,
            gateway_session_id,
            process_incarnation_id,
            telemetry_seq,
            telemetry_sample_id,
            reported_observed_unix,
        ),
        observed_at,
        state_started_at: Some(observed_at),
        causation_id: None,
        schedule_lineage: Vec::new(),
    })
}

/// Reconstructs the policy-facing traffic input for one exact projected
/// sample.  Activation and preview share this read-only cursor-fenced path, so
/// both see the same open-minute prefix as live evidence without taking a
/// traffic-stream lock or creating a second durable history owner.
pub(crate) async fn reconstruct_projected_policy_traffic_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    target_accepted_seq: i64,
    metrics: &AgentMetrics,
    network_admission_mask: &[u8],
    tunnel_admission_mask: &[u8],
) -> Result<TrafficAccountingRecord> {
    let observed_at = Utc
        .timestamp_opt(metrics.observed_unix.min(i64::MAX as u64) as i64, 0)
        .single()
        .context("projected policy traffic timestamp is invalid")?;
    let target_overlay = projected_traffic_counter_overlay_from_masks(
        metrics,
        network_admission_mask,
        tunnel_admission_mask,
    )?;
    let projected_streams = projected_traffic_streams(&target_overlay);
    let mut plan_ids = metrics
        .tunnels
        .iter()
        .filter_map(|tunnel| tunnel.plan_id.as_deref())
        .filter_map(|plan_id| Uuid::parse_str(plan_id.trim()).ok())
        .chain(
            metrics
                .tunnel_reachability
                .iter()
                .map(|observation| observation.plan_id),
        )
        .collect::<Vec<_>>();
    plan_ids.sort_unstable();
    plan_ids.dedup();
    let current_tunnel_plans =
        load_current_tunnel_plan_snapshot_for_ids_in_tx(tx, client_id, &plan_ids).await?;
    let mut traffic_context = load_projected_traffic_accounting_context_in_tx(
        tx,
        client_id,
        &current_tunnel_plans.managed_endpoint_interfaces,
    )
    .await?;
    let (_, traffic, _) = rebase_policy_traffic_frontier_in_tx(
        tx,
        &mut traffic_context,
        client_id,
        target_accepted_seq,
        observed_at,
        metrics,
        &projected_streams,
    )
    .await?;
    Ok(traffic)
}

/// Materializes the exact immutable accepted sample owned by one activation
/// work row.  Projection has already published its admission masks before the
/// work becomes claimable; no history/fleet scan and no rule evaluation occurs
/// inside this exact client owner.
pub(crate) async fn materialize_combined_telemetry_policy_baseline_sample_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    target_accepted_seq: i64,
    target_sample_id: Uuid,
) -> Result<bool> {
    let row = sqlx::query(
        r#"
        SELECT sample.payload,
               sample.source_gateway_session_id,
               sample.source_process_incarnation_id,
               sample.source_telemetry_seq,
               sample.reported_observed_unix,
               sample.network_admission_mask,
               sample.tunnel_admission_mask
        FROM telemetry_samples sample
        JOIN telemetry_projection_heads head ON head.client_id=sample.client_id
        WHERE sample.id=$1
          AND sample.client_id=$2
          AND sample.accepted_seq=$3
          AND head.projected_seq>=sample.accepted_seq
        "#,
    )
    .bind(target_sample_id)
    .bind(client_id)
    .bind(target_accepted_seq)
    .fetch_one(&mut **tx)
    .await
    .context("telemetry policy activation sample is not projected")?;
    let metrics = row.try_get::<SqlJson<AgentMetrics>, _>("payload")?.0;
    let gateway_session_id: Uuid = row.try_get("source_gateway_session_id")?;
    let process_incarnation_id: Uuid = row.try_get("source_process_incarnation_id")?;
    let telemetry_seq = u64::try_from(row.try_get::<i64, _>("source_telemetry_seq")?)
        .context("negative telemetry policy activation source sequence")?;
    let reported_observed_unix = u64::try_from(row.try_get::<i64, _>("reported_observed_unix")?)
        .context("negative telemetry policy activation reported time")?;
    let network_admission_mask: Vec<u8> = row.try_get("network_admission_mask")?;
    let tunnel_admission_mask: Vec<u8> = row.try_get("tunnel_admission_mask")?;
    let traffic = reconstruct_projected_policy_traffic_in_tx(
        tx,
        client_id,
        target_accepted_seq,
        &metrics,
        &network_admission_mask,
        &tunnel_admission_mask,
    )
    .await?;
    materialize_policy_evidence_baseline_in_tx(
        tx,
        combined_telemetry_policy_fact(
            client_id,
            gateway_session_id,
            process_incarnation_id,
            telemetry_seq,
            target_sample_id,
            reported_observed_unix,
            &metrics,
            &traffic,
        )?,
    )
    .await
}

pub(crate) fn combined_metric_evidence_payload(
    metrics: &AgentMetrics,
    traffic: &TrafficAccountingRecord,
    gateway_session_id: Uuid,
    process_incarnation_id: Uuid,
    telemetry_seq: u64,
    telemetry_sample_id: Uuid,
    reported_observed_unix: u64,
) -> Value {
    let disk = persistent_disk_totals(metrics).filter(|(total, _)| *total > 0);
    let memory_available_ratio = (metrics.memory.total_bytes > 0).then(|| {
        (metrics
            .memory
            .available_bytes
            .min(metrics.memory.total_bytes) as f64)
            / metrics.memory.total_bytes as f64
    });
    let disk_available_ratio =
        disk.map(|(total, available)| available.clamp(0, total) as f64 / total as f64);
    let cpu_load_saturation = (metrics.cpu.cores > 0)
        .then(|| metrics.cpu.load.one / f64::from(metrics.cpu.cores))
        .filter(|value| value.is_finite());
    let traffic_complete = (traffic.state == "ok").then_some(traffic);
    serde_json::json!({
        "telemetry": {
            "gateway_session_id": gateway_session_id,
            "process_incarnation_id": process_incarnation_id,
            "seq": telemetry_seq,
            "sample_id": telemetry_sample_id,
            "reported_observed_unix": reported_observed_unix,
            "accepted_observed_unix": metrics.observed_unix,
        },
        "cpu": {
            "utilization_ratio": metrics.cpu.utilization_ratio,
            "load_1": metrics.cpu.load.one,
            "load_saturation": cpu_load_saturation,
        },
        "memory": {"available_ratio": memory_available_ratio},
        "disk": {"available_ratio": disk_available_ratio},
        "traffic": {
            "quota": {
                "total": traffic_complete.and_then(|record| record.quota_total_bytes),
                "rx": traffic_complete.and_then(|record| record.quota_rx_bytes),
                "tx": traffic_complete.and_then(|record| record.quota_tx_bytes),
            },
            "cycle": {
                "total": traffic_complete.map(|record| record.total_bytes),
                "rx": traffic_complete.map(|record| record.rx_bytes),
                "tx": traffic_complete.map(|record| record.tx_bytes),
            },
            "cycle_percent": traffic_complete.and_then(|record| record.cycle_percent),
            "state": traffic.state,
            "snapshot": {
                "as_of_unix": metrics.observed_unix,
                "selector_hash": traffic.selector_hash,
                "last_sample_at": traffic.last_sample_at,
                "counter_epochs_seen": traffic.counter_epochs_seen,
            },
        },
    })
}

fn validate_deferred_telemetry_constraints(metrics: &AgentMetrics) -> Result<()> {
    let mut network_interfaces = HashSet::new();
    anyhow::ensure!(
        metrics
            .networks
            .iter()
            .all(|network| network_interfaces.insert(network.interface.as_str())),
        "telemetry networks contain a duplicate interface"
    );
    let mut interfaces = HashSet::new();
    anyhow::ensure!(
        metrics
            .tunnels
            .iter()
            .filter(|tunnel| structurally_valid_projected_telemetry_tunnel(tunnel))
            .all(|tunnel| interfaces.insert(tunnel.interface.as_str())),
        "telemetry tunnels contain a duplicate projected interface"
    );
    anyhow::ensure!(
        metrics
            .port_forwarding
            .as_ref()
            .is_none_or(|snapshot| snapshot.rules.len() <= vpsman_common::MAX_PORT_FORWARD_RULES),
        "port_forward_runtime_too_many_rules"
    );
    anyhow::ensure!(
        metrics.tunnel_reachability.iter().all(|observation| {
            observation.stale_after_secs <= i64::MAX as u64
                && observation.transmitted <= i32::MAX as u32
                && observation.received <= i32::MAX as u32
        }),
        "tunnel reachability counters exceed durable PostgreSQL representation"
    );
    Ok(())
}

fn valid_telemetry_name(value: &str) -> bool {
    let len = value.len();
    (1..=64).contains(&len)
}

fn sum_u64(values: impl Iterator<Item = u64>) -> i64 {
    values
        .fold(0_u128, |total, value| total.saturating_add(value as u128))
        .min(i64::MAX as u128) as i64
}

fn u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub(crate) async fn record_client_status_transition_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    from_status: Option<&str>,
    to_status: &str,
    reason: &str,
    mut metadata: serde_json::Value,
    origin_kind: &str,
    component: &str,
) -> Result<()> {
    let object = metadata
        .as_object_mut()
        .context("client status transition metadata must be an object")?;
    object.insert("result".to_string(), serde_json::json!(to_status));
    object.insert("origin_kind".to_string(), serde_json::json!(origin_kind));
    object.insert("component".to_string(), serde_json::json!(component));
    let webhook_metadata = metadata.clone();
    sqlx::query(
        r#"
        INSERT INTO client_status_history (
            id, client_id, from_status, to_status, reason, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(client_id)
    .bind(from_status)
    .bind(to_status)
    .bind(reason)
    .bind(metadata.clone())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(format!("agent.status_{to_status}"))
    .bind(format!("client:{client_id}"))
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    crate::repository_operational_alerts::reconcile_postgres_agent_alert_transition_in_tx(
        tx, client_id, to_status,
    )
    .await?;
    mark_postgres_tunnel_alerts_unknown_for_clients_in_tx(tx, &[client_id.to_string()]).await?;
    insert_client_status_webhook_event_in_tx(
        tx,
        client_id,
        from_status,
        to_status,
        reason,
        webhook_metadata,
    )
    .await?;
    Ok(())
}

pub(crate) async fn insert_client_status_webhook_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    from_status: Option<&str>,
    to_status: &str,
    reason: &str,
    metadata: serde_json::Value,
) -> Result<()> {
    let event_id = format!(
        "vps.status_changed:{client_id}:{to_status}:{}",
        Uuid::new_v4()
    );
    let event_predicates = vec![
        format!("vps.status.{to_status}"),
        format!("vps.status.become_{to_status}"),
    ];
    let subject_client_ids = vec![client_id.to_string()];
    let payload = serde_json::json!({
        "event": {
            "kind": "vps.status_changed",
            "from_status": from_status,
            "to_status": to_status,
            "reason": reason,
        },
        "vps_status": {
            "client_id": client_id,
            "from_status": from_status,
            "to_status": to_status,
            "reason": reason,
            "metadata": metadata,
        }
    });
    let occurred_at = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id,
            kind,
            event_id,
            event_predicates,
            subject_client_ids,
            payload,
            occurred_at,
            actor_id
        )
        VALUES ($1, 'vps.status_changed', $2, $3, $4, $5, $6::timestamptz, NULL)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&event_id)
    .bind(&event_predicates)
    .bind(&subject_client_ids)
    .bind(SqlJson(payload))
    .bind(occurred_at.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    let _ = sqlx::query("SELECT pg_notify('webhook_events', $1)")
        .bind(event_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
