use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use sqlx::{types::Json as SqlJson, Postgres, Row, Transaction};
use tokio::sync::RwLock;
use tracing::debug;
use uuid::Uuid;
use vpsman_common::{
    AgentHello, AgentMetrics, AgentUpdateHeartbeat, GatewayAgentHelloIngest,
    GatewaySessionLifecycleIngest, GatewayTelemetryIngest, JobCommand,
    RuntimeTunnelAdapterHealthStat, RuntimeTunnelStat,
};
use vpsman_server_core::{TARGET_STATUS_AGENT_LOST, TARGET_STATUS_COMPLETED, TARGET_STATUS_FAILED};

use crate::model::{
    AgentView, ClientStatusHistoryView, TelemetryNetworkRateView, TelemetryRollupView,
    TelemetrySampleView, TelemetryTunnelAdapterHealthView, TelemetryTunnelView,
};
use crate::model_alert_policies::TrafficCounterSampleRecord;
use crate::model_webhook_rules::WebhookEventCandidate;
use crate::repository::{Repository, TelemetryIngestWatermark, TelemetryIngestWatermarks};
use crate::repository_jobs::{
    append_synthetic_agent_lost_output_in_tx, append_synthetic_status_output_in_tx,
    enqueue_target_terminal_event_in_tx,
};
use crate::repository_key_lifecycle::public_key_sha256_hex;
use crate::repository_monitoring::{accepted_postgres_ping_results, upsert_postgres_ping_results};
use crate::repository_network_observations::reconcile_postgres_automatic_observation_series_for_client;
use crate::repository_network_traffic_import::{
    is_intentional_vnstat_import_boundary, lock_postgres_traffic_counter_streams,
};
use crate::security::constant_time_eq;

const TELEMETRY_BUCKET_SECS: i32 = 60;

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
                        anyhow::bail!("agent_update_activation_heartbeat_terminal_cas_lost:{job_id}:{target_client_id}");
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
                    anyhow::bail!("agent_update_activation_heartbeat_terminal_cas_lost:{job_id}:{target_client_id}");
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
            Self::Memory(memory) => {
                let _key_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let current_key_matches = memory
                    .client_public_keys
                    .read()
                    .await
                    .get(client_id)
                    .is_some_and(|expected| constant_time_eq(expected, &provided));
                let key_revoked = memory
                    .client_key_revocations
                    .read()
                    .await
                    .iter()
                    .any(|record| record.public_key_sha256_hex == provided_fingerprint);
                let identity_active = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .find(|agent| agent.id == client_id)
                    .is_some_and(|agent| !matches!(agent.status.as_str(), "revoked" | "deleted"));
                let hidden = memory.hidden_clients.read().await.contains(client_id);
                Ok(current_key_matches && !key_revoked && identity_active && !hidden)
            }
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
        let mut accepted_hello = true;
        let session_event = agent_hello_session_event(event);
        let authenticated_public_key =
            hex::decode(&event.noise_public_key_hex).with_context(|| {
                format!("invalid noise public key hex for {}", event.hello.client_id)
            })?;
        if authenticated_public_key.len() != 32 {
            return Ok(false);
        }
        match self {
            Self::Memory(memory) => {
                let _key_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let hidden = memory
                    .hidden_clients
                    .read()
                    .await
                    .contains(&event.hello.client_id);
                let fingerprint = public_key_sha256_hex(&authenticated_public_key);
                let current_key_matches = memory
                    .client_public_keys
                    .read()
                    .await
                    .get(&event.hello.client_id)
                    .is_some_and(|expected| constant_time_eq(expected, &authenticated_public_key));
                let key_revoked = memory
                    .client_key_revocations
                    .read()
                    .await
                    .iter()
                    .any(|record| record.public_key_sha256_hex == fingerprint);
                let identity_active = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .find(|agent| agent.id == event.hello.client_id)
                    .is_some_and(|agent| !matches!(agent.status.as_str(), "revoked" | "deleted"));
                let credential_accepted = current_key_matches && !key_revoked && identity_active;
                if !hidden && credential_accepted {
                    let prior = {
                        let agents = memory.agents.read().await;
                        agents
                            .iter()
                            .find(|agent| agent.id == event.hello.client_id)
                            .map(|agent| {
                                (
                                    agent.status.clone(),
                                    agent.internal_build_number,
                                    agent.stale_reason.clone(),
                                )
                            })
                    };
                    upsert_memory_agent_with_remote_ip(
                        &memory.agents,
                        &event.hello,
                        event.remote_ip.as_deref(),
                    )
                    .await;
                    memory.client_system_facts.write().await.insert(
                        event.hello.client_id.clone(),
                        crate::model::ClientSystemFactsRecord {
                            os_release: event.hello.os_release.clone(),
                            architecture: event.hello.arch.clone(),
                            cpu_model: event.hello.cpu_model.clone(),
                            kernel_release: event.hello.kernel_release.clone(),
                            virtualization: event.hello.virtualization.clone(),
                            reported_at: crate::unix_now().to_string(),
                        },
                    );
                    crate::repository_gateway_sessions::expire_memory_active_other_sessions(
                        memory,
                        &event.hello.client_id,
                        event.gateway_session_id,
                    )
                    .await;
                    crate::repository_gateway_sessions::upsert_memory_gateway_session(
                        memory,
                        &session_event,
                        "active",
                        None,
                    )
                    .await;
                    if let Some((prior_status, prior_build, stale_reason)) = prior {
                        let resulting_status = memory
                            .agents
                            .read()
                            .await
                            .iter()
                            .find(|agent| agent.id == event.hello.client_id)
                            .map(|agent| agent.status.clone())
                            .unwrap_or(prior_status.clone());
                        if prior_status != resulting_status {
                            let reason = if prior_status == "never" {
                                "agent_first_connection"
                            } else if prior_status == "stale" {
                                "agent_reconnected_with_changed_internal_build"
                            } else {
                                "agent_reconnected"
                            };
                            let now = crate::unix_now().to_string();
                            let metadata = serde_json::json!({
                                "from_status": &prior_status,
                                "to_status": &resulting_status,
                                "reason": reason,
                                "stale_reason": stale_reason,
                                "previous_internal_build_number": prior_build,
                                "internal_build_number": event.hello.internal_build_number,
                                "gateway_id": &event.gateway_id,
                                "result": &resulting_status,
                                "origin_kind": "gateway_ingest",
                                "component": "agent-ingest",
                            });
                            memory.client_status_history.write().await.push(
                                ClientStatusHistoryView {
                                    id: Uuid::new_v4(),
                                    client_id: event.hello.client_id.clone(),
                                    from_status: Some(prior_status.clone()),
                                    to_status: resulting_status.clone(),
                                    reason: reason.to_string(),
                                    metadata: metadata.clone(),
                                    created_at: now.clone(),
                                },
                            );
                            memory
                                .audits
                                .write()
                                .await
                                .push(crate::model::AuditLogView {
                                    id: Uuid::new_v4(),
                                    actor_id: None,
                                    action: format!("agent.status_{resulting_status}"),
                                    target: format!("client:{}", event.hello.client_id),
                                    command_hash: None,
                                    metadata: metadata.clone(),
                                    created_at: now,
                                });
                            self.record_client_status_webhook_event(
                                &event.hello.client_id,
                                Some(&prior_status),
                                &resulting_status,
                                reason,
                                metadata,
                            )
                            .await?;
                        }
                    }
                } else {
                    accepted_hello = false;
                }
            }
            Self::Postgres(pool) => {
                crate::repository_webhook_rules::ensure_webhook_event_partition(pool, Utc::now())
                    .await?;
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
                        END
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
                let mut agent_lost_job_ids = Vec::new();
                if accepted_hello && process_incarnation_changed {
                    if let Some(previous_process_incarnation_id) = prior_process_incarnation_id {
                        agent_lost_job_ids = mark_old_incarnation_targets_agent_lost_in_tx(
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
                }

                tx.commit().await?;
                for job_id in agent_lost_job_ids {
                    let _ = self.refresh_job_status_from_targets(job_id).await?;
                }
            }
        }
        if accepted_hello {
            if let Some(heartbeat) = update_heartbeat.as_ref() {
                debug!(
                    client_id = %event.hello.client_id,
                    activation_job_id = %heartbeat.activation_job_id,
                    sha256_hex = %heartbeat.sha256_hex,
                    "recording agent update heartbeat"
                );
                self.record_agent_update_heartbeat(&event.hello.client_id, heartbeat)
                    .await?;
            }
        }
        Ok(accepted_hello)
    }

    pub(crate) async fn record_telemetry(&self, event: &GatewayTelemetryIngest) -> Result<bool> {
        let mut received_metrics = event.telemetry.metrics.clone();
        let reported_observed_unix = received_metrics.observed_unix;
        let received_unix = crate::unix_now();
        let mut ping_source_checked_unix = Vec::with_capacity(received_metrics.ping_results.len());
        for result in &mut received_metrics.ping_results {
            let source_checked_unix = result.checked_unix;
            let check_age = reported_observed_unix.saturating_sub(result.checked_unix);
            result.checked_unix = received_unix.saturating_sub(check_age);
            // The source timestamp is the stable identity of a logical probe.
            // The rebased timestamp remains the trusted chart timestamp, but it
            // can move by a second when an unchanged cached result is received
            // again with different transport latency.
            ping_source_checked_unix.push(source_checked_unix);
        }
        for observation in &mut received_metrics.tunnel_reachability {
            let measurement_age = reported_observed_unix.saturating_sub(observation.measured_unix);
            observation.measured_unix = received_unix.saturating_sub(measurement_age);
        }
        received_metrics.observed_unix = received_unix;
        let swap = validated_swap_sample(&received_metrics)?;
        let record_result: Result<bool> = match self {
            Self::Memory(memory) => {
                let _key_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                if memory
                    .hidden_clients
                    .read()
                    .await
                    .contains(&event.telemetry.client_id)
                {
                    return Ok(false);
                }
                let active_identity = memory.agents.read().await.iter().any(|agent| {
                    agent.id == event.telemetry.client_id
                        && !matches!(agent.status.as_str(), "revoked" | "deleted")
                        && agent.process_incarnation_id == Some(event.process_incarnation_id)
                });
                let active_session = memory.gateway_sessions.read().await.iter().any(|session| {
                    session.gateway_id == event.gateway_id
                        && session.client_id == event.telemetry.client_id
                        && session.id == event.gateway_session_id
                        && session.status == "active"
                });
                if !active_identity || !active_session {
                    return Ok(false);
                }
                match claim_memory_telemetry_sequence(
                    &memory.telemetry_ingest_watermarks,
                    &event.telemetry.client_id,
                    event.gateway_session_id,
                    event.process_incarnation_id,
                    event.telemetry_seq,
                )
                .await
                {
                    TelemetrySequenceClaim::Accepted => {}
                    TelemetrySequenceClaim::Duplicate => {
                        drop(_key_lifecycle_guard);
                        self.record_automatic_tunnel_reachability(
                            &event.telemetry.client_id,
                            &received_metrics.tunnel_reachability,
                        )
                        .await?;
                        self.record_port_forward_runtime_from_telemetry(
                            &event.telemetry.client_id,
                            &received_metrics,
                        )
                        .await?;
                        self.record_telemetry_webhook_event(event).await?;
                        return Ok(false);
                    }
                    TelemetrySequenceClaim::Stale => return Ok(false),
                }
                touch_memory_agent_from_telemetry(
                    &memory.agents,
                    &event.telemetry.client_id,
                    event.remote_ip.as_deref(),
                )
                .await;
                let (accepted_ping_results, accepted_ping_source_checked_unix) = self
                    .accepted_ping_results_memory(
                        &event.telemetry.client_id,
                        received_metrics.observed_unix,
                        &received_metrics.ping_results,
                        &ping_source_checked_unix,
                    )
                    .await?;
                received_metrics.ping_results = accepted_ping_results;
                upsert_memory_telemetry_sample(
                    &memory.telemetry_samples,
                    Uuid::new_v4(),
                    &event.telemetry.client_id,
                    &received_metrics,
                )
                .await?;
                upsert_memory_telemetry_rollup(
                    &memory.telemetry_rollups,
                    &event.telemetry.client_id,
                    &received_metrics,
                    swap,
                )
                .await;
                upsert_memory_traffic_counter_samples(
                    &memory.traffic_counter_samples,
                    &event.telemetry.client_id,
                    &received_metrics,
                )
                .await;
                upsert_memory_telemetry_network_rates(
                    &memory.telemetry_network_rates,
                    &memory.traffic_counter_samples,
                    &event.telemetry.client_id,
                    &received_metrics,
                )
                .await;
                self.record_ping_results_memory(
                    &event.telemetry.client_id,
                    received_metrics.observed_unix,
                    &received_metrics.ping_results,
                    &accepted_ping_source_checked_unix,
                )
                .await?;
                let mut tunnels = memory.telemetry_tunnels.write().await;
                tunnels.retain(|record| record.client_id != event.telemetry.client_id);
                tunnels.extend(received_metrics.tunnels.iter().filter_map(|tunnel| {
                    telemetry_tunnel_view(
                        &event.telemetry.client_id,
                        received_metrics.observed_unix,
                        tunnel,
                    )
                }));
                Ok(true)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let visible_client = sqlx::query_scalar::<_, String>(
                    r#"
                    SELECT client.id
                    FROM visible_clients client
                    WHERE client.id = $1
                      AND client.status <> 'revoked'
                      AND client.process_incarnation_id = $2
                      AND EXISTS (
                          SELECT 1
                          FROM gateway_sessions session
                          WHERE session.gateway_id = $3
                            AND session.client_id = client.id
                            AND session.id = $4
                            AND session.status = 'active'
                      )
                    FOR UPDATE
                    "#,
                )
                .bind(&event.telemetry.client_id)
                .bind(event.process_incarnation_id)
                .bind(&event.gateway_id)
                .bind(event.gateway_session_id)
                .fetch_optional(&mut *tx)
                .await?;
                if visible_client.is_none() {
                    tx.commit().await?;
                    return Ok(false);
                }
                match claim_postgres_telemetry_sequence(&mut tx, event).await? {
                    TelemetrySequenceClaim::Accepted => {}
                    TelemetrySequenceClaim::Duplicate => {
                        tx.commit().await?;
                        self.record_automatic_tunnel_reachability(
                            &event.telemetry.client_id,
                            &received_metrics.tunnel_reachability,
                        )
                        .await?;
                        self.record_port_forward_runtime_from_telemetry(
                            &event.telemetry.client_id,
                            &received_metrics,
                        )
                        .await?;
                        self.record_telemetry_webhook_event(event).await?;
                        return Ok(false);
                    }
                    TelemetrySequenceClaim::Stale => {
                        tx.commit().await?;
                        return Ok(false);
                    }
                }
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
                lock_postgres_traffic_counter_streams(&mut tx, &event.telemetry.client_id).await?;
                let metrics = &received_metrics;
                let sample_id = Uuid::new_v4();
                upsert_postgres_telemetry_sample(
                    &mut tx,
                    sample_id,
                    &event.telemetry.client_id,
                    metrics,
                )
                .await?;
                insert_postgres_telemetry_counter_facts(
                    &mut tx,
                    sample_id,
                    &event.telemetry.client_id,
                    metrics,
                )
                .await?;
                insert_postgres_telemetry_ping_facts(
                    &mut tx,
                    sample_id,
                    &event.telemetry.client_id,
                    metrics,
                    &accepted_ping_source_checked_unix,
                )
                .await?;
                upsert_postgres_telemetry_rollup(
                    &mut tx,
                    &event.telemetry.client_id,
                    metrics,
                    swap,
                )
                .await?;
                upsert_postgres_traffic_counter_samples(
                    &mut tx,
                    &event.telemetry.client_id,
                    metrics,
                )
                .await?;
                upsert_postgres_telemetry_network_rates(
                    &mut tx,
                    &event.telemetry.client_id,
                    metrics,
                )
                .await?;
                upsert_postgres_ping_results(
                    &mut tx,
                    &event.telemetry.client_id,
                    metrics.observed_unix,
                    &metrics.ping_results,
                    &accepted_ping_source_checked_unix,
                )
                .await?;
                upsert_postgres_telemetry_tunnels(&mut tx, &event.telemetry.client_id, metrics)
                    .await?;
                sqlx::query(
                    r#"
                    UPDATE clients
                    SET
                        status = CASE WHEN status = 'stale' THEN status ELSE 'online' END,
                        registration_ip = COALESCE(registration_ip, $2::inet),
                        last_ip = COALESCE($2::inet, last_ip),
                        last_seen_at = now()
                    WHERE id = $1 AND hidden_at IS NULL
                      AND status <> 'revoked'
                    "#,
                )
                .bind(&event.telemetry.client_id)
                .bind(event.remote_ip.as_deref())
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(true)
            }
        };
        let recorded = record_result?;
        if !recorded {
            return Ok(false);
        }
        self.record_automatic_tunnel_reachability(
            &event.telemetry.client_id,
            &received_metrics.tunnel_reachability,
        )
        .await?;
        self.record_port_forward_runtime_from_telemetry(
            &event.telemetry.client_id,
            &received_metrics,
        )
        .await?;
        self.record_telemetry_webhook_event(event).await?;
        Ok(true)
    }

    async fn record_port_forward_runtime_from_telemetry(
        &self,
        client_id: &str,
        metrics: &vpsman_common::AgentMetrics,
    ) -> Result<()> {
        if let Some(snapshot) = metrics.port_forwarding.as_ref() {
            let mut snapshot = snapshot.clone();
            snapshot.observed_unix = metrics.observed_unix;
            self.record_port_forward_runtime_snapshot(client_id, &snapshot)
                .await?;
        }
        Ok(())
    }

    async fn record_telemetry_webhook_event(&self, event: &GatewayTelemetryIngest) -> Result<()> {
        let metrics = &event.telemetry.metrics;
        let mut predicates = vec!["telemetry.rollup".to_string()];
        if !metrics.networks.is_empty() {
            predicates.push("telemetry.network_rate".to_string());
        }
        if !metrics.tunnels.is_empty() {
            predicates.push("telemetry.tunnel".to_string());
        }
        if !metrics.tunnel_reachability.is_empty() {
            predicates.push("network.reachability".to_string());
        }
        predicates.sort();
        predicates.dedup();
        let (disk_total, disk_available, network_rx, network_tx) = telemetry_totals(metrics);
        let event_id = format!(
            "telemetry:{}:{}:{}:{}",
            event.telemetry.client_id,
            event.gateway_session_id,
            event.process_incarnation_id,
            event.telemetry_seq
        );
        self.record_webhook_event(WebhookEventCandidate {
            kind: "telemetry.rollup".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: vec![event.telemetry.client_id.clone()],
            actor_id: None,
            payload: serde_json::json!({
                "event": {
                    "kind": "telemetry.rollup",
                    "id": &event_id,
                    "predicates": &predicates,
                },
                "telemetry": {
                    "client_id": &event.telemetry.client_id,
                    "gateway_id": &event.gateway_id,
                    "observed_unix": metrics.observed_unix,
                    "hostname": &metrics.hostname,
                    "uptime_secs": metrics.uptime_secs,
                    "disk_total_bytes": disk_total,
                    "disk_available_bytes": disk_available,
                    "network_rx_bytes": network_rx,
                    "network_tx_bytes": network_tx,
                    "network_count": metrics.networks.len(),
                    "tunnel_count": metrics.tunnels.len(),
                    "networks": &metrics.networks,
                    "tunnels": &metrics.tunnels,
                },
            }),
        })
        .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn mark_agent_stale(
        &self,
        client_id: &str,
        reason: &str,
        metadata: serde_json::Value,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut agents = memory.agents.write().await;
                if let Some(agent) = agents.iter_mut().find(|agent| agent.id == client_id) {
                    if matches!(agent.status.as_str(), "revoked" | "deleted") {
                        return Ok(());
                    }
                    if agent.status != "stale" {
                        let from_status = agent.status.clone();
                        agent.status = "stale".to_string();
                        agent.stale_since = Some(crate::unix_now().to_string());
                        agent.stale_reason = Some(reason.to_string());
                        let webhook_metadata = serde_json::json!({
                            "reason": reason,
                            "details": metadata,
                        });
                        drop(agents);
                        memory
                            .audits
                            .write()
                            .await
                            .push(crate::model::AuditLogView {
                                id: Uuid::new_v4(),
                                actor_id: None,
                                action: "agent.status_stale".to_string(),
                                target: format!("client:{client_id}"),
                                command_hash: None,
                                    metadata: serde_json::json!({
                                        "from_status": from_status,
                                        "to_status": "stale",
                                        "reason": reason,
                                        "details": webhook_metadata.get("details").cloned().unwrap_or(serde_json::Value::Null),
                                        "result": "stale",
                                        "origin_kind": "control_plane",
                                        "component": "agent-status-tracker",
                                    }),
                                    created_at: crate::unix_now().to_string(),
                                });
                        self.record_client_status_webhook_event(
                            client_id,
                            Some(&from_status),
                            "stale",
                            reason,
                            webhook_metadata,
                        )
                        .await?;
                    }
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                crate::repository_webhook_rules::ensure_webhook_event_partition(pool, Utc::now())
                    .await?;
                let mut tx = pool.begin().await?;
                let prior = sqlx::query(
                    r#"
                    SELECT status, internal_build_number
                    FROM visible_clients
                    WHERE id = $1 AND status <> 'revoked'
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(prior) = prior else {
                    tx.commit().await?;
                    return Ok(());
                };
                let from_status: String = prior.try_get("status")?;
                let internal_build_number =
                    prior.try_get::<i64, _>("internal_build_number")?.max(1);
                sqlx::query(
                    r#"
                    UPDATE clients
                    SET
                        status = 'stale',
                        stale_since = COALESCE(stale_since, now()),
                        stale_reason = $2,
                        stale_build_number = COALESCE(stale_build_number, internal_build_number)
                    WHERE id = $1 AND hidden_at IS NULL
                    "#,
                )
                .bind(client_id)
                .bind(reason)
                .execute(&mut *tx)
                .await?;
                if from_status != "stale" {
                    let metadata = serde_json::json!({
                        "reason": reason,
                        "internal_build_number": internal_build_number,
                        "details": metadata,
                    });
                    record_client_status_transition_in_tx(
                        &mut tx,
                        client_id,
                        Some(&from_status),
                        "stale",
                        reason,
                        metadata,
                        "control_plane",
                        "agent-status-tracker",
                    )
                    .await?;
                }
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn record_client_status_webhook_event(
        &self,
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
        self.record_webhook_event(WebhookEventCandidate {
            kind: "vps.status_changed".to_string(),
            event_id,
            event_predicates: vec![
                format!("vps.status.{to_status}"),
                format!("vps.status.become_{to_status}"),
            ],
            subject_client_ids: vec![client_id.to_string()],
            payload: serde_json::json!({
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
            }),
            actor_id: None,
        })
        .await?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TelemetrySequenceClaim {
    Accepted,
    Duplicate,
    Stale,
}

async fn claim_memory_telemetry_sequence(
    watermarks: &TelemetryIngestWatermarks,
    client_id: &str,
    gateway_session_id: Uuid,
    process_incarnation_id: Uuid,
    telemetry_seq: u64,
) -> TelemetrySequenceClaim {
    let mut watermarks = watermarks.write().await;
    if let Some(watermark) = watermarks.get(client_id) {
        if watermark.gateway_session_id == gateway_session_id
            && watermark.process_incarnation_id == process_incarnation_id
        {
            if watermark.telemetry_seq == telemetry_seq {
                return TelemetrySequenceClaim::Duplicate;
            }
            if watermark.telemetry_seq > telemetry_seq {
                return TelemetrySequenceClaim::Stale;
            }
        }
    }
    watermarks.insert(
        client_id.to_string(),
        TelemetryIngestWatermark {
            gateway_session_id,
            process_incarnation_id,
            telemetry_seq,
        },
    );
    TelemetrySequenceClaim::Accepted
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

async fn upsert_memory_telemetry_sample(
    samples: &Arc<RwLock<Vec<TelemetrySampleView>>>,
    id: Uuid,
    client_id: &str,
    metrics: &AgentMetrics,
) -> Result<()> {
    let observed_at = metrics.observed_unix.to_string();
    let sample = TelemetrySampleView {
        id,
        client_id: client_id.to_string(),
        observed_at: observed_at.clone(),
        cpu_load_1: metrics.cpu.load.one,
        memory_total_bytes: u64_to_i64(metrics.memory.total_bytes),
        memory_available_bytes: u64_to_i64(metrics.memory.available_bytes),
        payload: serde_json::to_value(metrics)?,
    };
    samples.write().await.push(sample);
    Ok(())
}

async fn upsert_postgres_telemetry_sample(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    client_id: &str,
    metrics: &AgentMetrics,
) -> Result<()> {
    let (disk_total, disk_available, network_rx, network_tx) = telemetry_totals(metrics);
    /*
     * The historical PostgreSQL raw projection treated a missing connection
     * snapshot as the saturated BIGINT sentinel. Retain that observable value
     * in the typed projection instead of silently converting it to zero.
     */
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
            network_rx_bytes,
            network_tx_bytes,
            tcp_sockets,
            udp_sockets,
            payload
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
            $19
        )
        "#,
    )
    .bind(id)
    .bind(client_id)
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
    .bind(disk_total)
    .bind(disk_available)
    .bind(network_rx)
    .bind(network_tx)
    .bind(tcp_sockets)
    .bind(udp_sockets)
    .bind(SqlJson(metrics))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_postgres_telemetry_counter_facts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sample_id: Uuid,
    client_id: &str,
    metrics: &AgentMetrics,
) -> Result<()> {
    let mut source_kinds = Vec::with_capacity(metrics.networks.len() + metrics.tunnels.len());
    let mut ordinals = Vec::with_capacity(source_kinds.capacity());
    let mut interfaces = Vec::with_capacity(source_kinds.capacity());
    let mut rx_bytes = Vec::with_capacity(source_kinds.capacity());
    let mut tx_bytes = Vec::with_capacity(source_kinds.capacity());

    for (ordinal, network) in metrics.networks.iter().enumerate() {
        source_kinds.push("host");
        ordinals.push(i32::try_from(ordinal).unwrap_or(i32::MAX));
        interfaces.push(network.interface.as_str());
        rx_bytes.push(u64_to_i64(network.rx_bytes));
        tx_bytes.push(u64_to_i64(network.tx_bytes));
    }
    for (ordinal, tunnel) in metrics.tunnels.iter().enumerate() {
        source_kinds.push("tunnel");
        ordinals.push(i32::try_from(ordinal).unwrap_or(i32::MAX));
        interfaces.push(tunnel.interface.as_str());
        rx_bytes.push(u64_to_i64(tunnel.rx_bytes));
        tx_bytes.push(u64_to_i64(tunnel.tx_bytes));
    }
    if source_kinds.is_empty() {
        return Ok(());
    }

    sqlx::query(
        r#"
        INSERT INTO telemetry_counter_facts (
            sample_id,
            client_id,
            observed_at,
            source_kind,
            ordinal,
            interface,
            rx_bytes,
            tx_bytes
        )
        SELECT
            $1,
            $2,
            to_timestamp($3::double precision),
            fact.source_kind,
            fact.ordinal,
            fact.interface,
            fact.rx_bytes,
            fact.tx_bytes
        FROM UNNEST(
            $4::TEXT[],
            $5::INTEGER[],
            $6::TEXT[],
            $7::BIGINT[],
            $8::BIGINT[]
        ) AS fact(source_kind, ordinal, interface, rx_bytes, tx_bytes)
        "#,
    )
    .bind(sample_id)
    .bind(client_id)
    .bind(metrics.observed_unix as f64)
    .bind(&source_kinds)
    .bind(&ordinals)
    .bind(&interfaces)
    .bind(&rx_bytes)
    .bind(&tx_bytes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_postgres_telemetry_ping_facts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sample_id: Uuid,
    client_id: &str,
    metrics: &AgentMetrics,
    source_checked_unix: &[u64],
) -> Result<()> {
    if metrics.ping_results.is_empty() {
        return Ok(());
    }

    let ordinals = metrics
        .ping_results
        .iter()
        .enumerate()
        .map(|(ordinal, _)| i32::try_from(ordinal).unwrap_or(i32::MAX))
        .collect::<Vec<_>>();
    let target_ids = metrics
        .ping_results
        .iter()
        .map(|result| Uuid::parse_str(result.target_id.trim()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let generations = metrics
        .ping_results
        .iter()
        .map(|result| u64_to_i64(result.generation))
        .collect::<Vec<_>>();
    let checked_unix = metrics
        .ping_results
        .iter()
        .map(|result| u64_to_i64(result.checked_unix))
        .collect::<Vec<_>>();
    let source_checked_unix = source_checked_unix
        .iter()
        .copied()
        .map(u64_to_i64)
        .collect::<Vec<_>>();
    let statuses = metrics
        .ping_results
        .iter()
        .map(|result| result.status.as_str())
        .collect::<Vec<_>>();
    let latency_avg_ms = metrics
        .ping_results
        .iter()
        .map(|result| result.latency_avg_ms)
        .collect::<Vec<_>>();
    let loss_ratios = metrics
        .ping_results
        .iter()
        .map(|result| result.loss_ratio)
        .collect::<Vec<_>>();
    let reasons = metrics
        .ping_results
        .iter()
        .map(|result| result.reason.as_deref())
        .collect::<Vec<_>>();

    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_series (client_id, target_id, generation)
        SELECT DISTINCT $1, fact.target_id, fact.generation
        FROM UNNEST($2::UUID[], $3::BIGINT[]) AS fact(target_id, generation)
        ON CONFLICT (client_id, target_id, generation) DO NOTHING
        "#,
    )
    .bind(client_id)
    .bind(&target_ids)
    .bind(&generations)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_facts (
            series_id,
            observed_at,
            evidence_id,
            source_checked_unix,
            checked_unix,
            status,
            latency_avg_ms,
            loss_ratio,
            reason
        )
        SELECT
            series.id,
            to_timestamp($3::double precision),
            $1,
            fact.source_checked_unix,
            fact.checked_unix,
            fact.status,
            fact.latency_avg_ms,
            fact.loss_ratio,
            fact.reason
        FROM (
            SELECT DISTINCT ON (target_id, generation, source_checked_unix) *
            FROM UNNEST(
                $4::INTEGER[],
                $5::UUID[],
                $6::BIGINT[],
                $7::BIGINT[],
                $8::BIGINT[],
                $9::TEXT[],
                $10::DOUBLE PRECISION[],
                $11::DOUBLE PRECISION[],
                $12::TEXT[]
            ) AS input(
                ordinal,
                target_id,
                generation,
                source_checked_unix,
                checked_unix,
                status,
                latency_avg_ms,
                loss_ratio,
                reason
            )
            ORDER BY target_id, generation, source_checked_unix, ordinal DESC
        ) fact
        JOIN telemetry_ping_series series
          ON series.client_id = $2
         AND series.target_id = fact.target_id
         AND series.generation = fact.generation
        WHERE fact.checked_unix <= floor($3::double precision)::bigint + 300
          AND floor($3::double precision)::bigint - fact.checked_unix <= 3900
        ON CONFLICT (series_id, source_checked_unix) DO UPDATE SET
            evidence_id = EXCLUDED.evidence_id,
            status = EXCLUDED.status,
            latency_avg_ms = EXCLUDED.latency_avg_ms,
            loss_ratio = EXCLUDED.loss_ratio,
            reason = EXCLUDED.reason
        "#,
    )
    .bind(sample_id)
    .bind(client_id)
    .bind(metrics.observed_unix as f64)
    .bind(&ordinals)
    .bind(&target_ids)
    .bind(&generations)
    .bind(&source_checked_unix)
    .bind(&checked_unix)
    .bind(&statuses)
    .bind(&latency_avg_ms)
    .bind(&loss_ratios)
    .bind(&reasons)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_memory_telemetry_rollup(
    rollups: &Arc<RwLock<Vec<TelemetryRollupView>>>,
    client_id: &str,
    metrics: &AgentMetrics,
    swap: Option<(i64, i64)>,
) {
    let bucket_start = bucket_start_unix(metrics.observed_unix).to_string();
    let observed_at = metrics.observed_unix.to_string();
    let (disk_total, disk_available, network_rx, network_tx) = telemetry_totals(metrics);
    let memory_total = u64_to_i64(metrics.memory.total_bytes);
    let memory_available = u64_to_i64(metrics.memory.available_bytes);
    let memory_used_ratio = resource_used_ratio_or_zero(memory_total, memory_available);
    let disk_used_ratio = resource_used_ratio_or_zero(disk_total, disk_available);
    let positive_swap = swap.filter(|(total, _)| *total > 0);
    let mut rollups = rollups.write().await;
    if let Some(rollup) = rollups.iter_mut().find(|rollup| {
        rollup.client_id == client_id
            && rollup.bucket_secs == TELEMETRY_BUCKET_SECS
            && rollup.bucket_start == bucket_start
    }) {
        let current_count = rollup.sample_count.max(1);
        rollup.sample_count = rollup.sample_count.saturating_add(1);
        if let Some(cpu_usage) = metrics.cpu.utilization_ratio {
            let usage_count = rollup.cpu_usage_sample_count.max(0);
            rollup.cpu_usage_avg = Some(match rollup.cpu_usage_avg {
                Some(current) if usage_count > 0 => {
                    weighted_avg_f64(current, usage_count, cpu_usage)
                }
                _ => cpu_usage,
            });
            rollup.cpu_usage_max = Some(
                rollup
                    .cpu_usage_max
                    .map_or(cpu_usage, |current| current.max(cpu_usage)),
            );
            rollup.cpu_usage_sample_count = usage_count.saturating_add(1);
        }
        rollup.cpu_cores_max = rollup.cpu_cores_max.max(i32::from(metrics.cpu.cores));
        rollup.cpu_load_1_avg =
            weighted_avg_f64(rollup.cpu_load_1_avg, current_count, metrics.cpu.load.one);
        rollup.cpu_load_1_max = rollup.cpu_load_1_max.max(metrics.cpu.load.one);
        rollup.cpu_load_5_avg =
            weighted_avg_f64(rollup.cpu_load_5_avg, current_count, metrics.cpu.load.five);
        rollup.cpu_load_5_max = rollup.cpu_load_5_max.max(metrics.cpu.load.five);
        rollup.cpu_load_15_avg = weighted_avg_f64(
            rollup.cpu_load_15_avg,
            current_count,
            metrics.cpu.load.fifteen,
        );
        rollup.cpu_load_15_max = rollup.cpu_load_15_max.max(metrics.cpu.load.fifteen);
        rollup.memory_total_bytes_max = rollup.memory_total_bytes_max.max(memory_total);
        rollup.memory_available_bytes_avg = weighted_avg_i64(
            rollup.memory_available_bytes_avg,
            current_count,
            memory_available,
        );
        rollup.memory_available_bytes_min = rollup.memory_available_bytes_min.min(memory_available);
        rollup.memory_used_ratio_avg = weighted_avg_f64(
            rollup.memory_used_ratio_avg,
            current_count,
            memory_used_ratio,
        );
        rollup.memory_used_ratio_max = rollup.memory_used_ratio_max.max(memory_used_ratio);
        if let Some((swap_total, swap_available)) = swap {
            let swap_count = rollup.swap_sample_count.max(0);
            rollup.swap_total_bytes_max = Some(
                rollup
                    .swap_total_bytes_max
                    .map_or(swap_total, |current| current.max(swap_total)),
            );
            if swap_total == 0 {
                if swap_count == 0 {
                    rollup.swap_available_bytes_avg = Some(0);
                    rollup.swap_available_bytes_min = Some(0);
                    rollup.swap_used_ratio_avg = None;
                    rollup.swap_used_ratio_max = None;
                }
            } else {
                let swap_used_ratio = resource_used_ratio(swap_total, swap_available);
                rollup.swap_available_bytes_avg = Some(match rollup.swap_available_bytes_avg {
                    Some(current) if swap_count > 0 => {
                        weighted_avg_i64(current, swap_count, swap_available)
                    }
                    _ => swap_available,
                });
                rollup.swap_available_bytes_min = Some(match rollup.swap_available_bytes_min {
                    Some(current) if swap_count > 0 => current.min(swap_available),
                    _ => swap_available,
                });
                rollup.swap_used_ratio_avg = Some(match rollup.swap_used_ratio_avg {
                    Some(current) if swap_count > 0 => {
                        weighted_avg_f64(current, swap_count, swap_used_ratio)
                    }
                    _ => swap_used_ratio,
                });
                rollup.swap_used_ratio_max = Some(
                    rollup
                        .swap_used_ratio_max
                        .map_or(swap_used_ratio, |current| current.max(swap_used_ratio)),
                );
                rollup.swap_sample_count = swap_count.saturating_add(1);
            }
        }
        rollup.disk_total_bytes_max = rollup.disk_total_bytes_max.max(disk_total);
        rollup.disk_available_bytes_avg = weighted_avg_i64(
            rollup.disk_available_bytes_avg,
            current_count,
            disk_available,
        );
        rollup.disk_available_bytes_min = rollup.disk_available_bytes_min.min(disk_available);
        rollup.disk_used_ratio_avg =
            weighted_avg_f64(rollup.disk_used_ratio_avg, current_count, disk_used_ratio);
        rollup.disk_used_ratio_max = rollup.disk_used_ratio_max.max(disk_used_ratio);
        rollup.network_rx_bytes_max = rollup.network_rx_bytes_max.max(network_rx);
        rollup.network_tx_bytes_max = rollup.network_tx_bytes_max.max(network_tx);
        if let Some(connections) = metrics.connections.as_ref() {
            rollup.connections_sample_count = rollup.connections_sample_count.saturating_add(1);
            if rollup
                .connections_observed_at
                .as_deref()
                .map(parse_unix)
                .is_none_or(|stored| metrics.observed_unix >= stored)
            {
                rollup.tcp_sockets_latest = Some(u64_to_i64(connections.tcp));
                rollup.udp_sockets_latest = Some(u64_to_i64(connections.udp));
                rollup.connections_observed_at = Some(observed_at.clone());
            }
        }
        if metrics.observed_unix >= parse_unix(&rollup.latest_observed_at) {
            rollup.latest_observed_at = observed_at.clone();
        }
        rollup.updated_at = observed_at;
        return;
    }

    rollups.push(TelemetryRollupView {
        client_id: client_id.to_string(),
        bucket_start,
        bucket_secs: TELEMETRY_BUCKET_SECS,
        sample_count: 1,
        cpu_usage_sample_count: i32::from(metrics.cpu.utilization_ratio.is_some()),
        cpu_usage_avg: metrics.cpu.utilization_ratio,
        cpu_usage_max: metrics.cpu.utilization_ratio,
        cpu_cores_max: i32::from(metrics.cpu.cores),
        cpu_load_1_avg: metrics.cpu.load.one,
        cpu_load_1_max: metrics.cpu.load.one,
        cpu_load_5_avg: metrics.cpu.load.five,
        cpu_load_5_max: metrics.cpu.load.five,
        cpu_load_15_avg: metrics.cpu.load.fifteen,
        cpu_load_15_max: metrics.cpu.load.fifteen,
        memory_total_bytes_max: memory_total,
        memory_available_bytes_avg: memory_available,
        memory_available_bytes_min: memory_available,
        memory_used_ratio_avg: memory_used_ratio,
        memory_used_ratio_max: memory_used_ratio,
        swap_sample_count: i32::from(positive_swap.is_some()),
        swap_total_bytes_max: swap.map(|(total, _)| total),
        swap_available_bytes_avg: swap.map(|(_, available)| available),
        swap_available_bytes_min: swap.map(|(_, available)| available),
        swap_used_ratio_avg: positive_swap
            .map(|(total, available)| resource_used_ratio(total, available)),
        swap_used_ratio_max: positive_swap
            .map(|(total, available)| resource_used_ratio(total, available)),
        disk_total_bytes_max: disk_total,
        disk_available_bytes_avg: disk_available,
        disk_available_bytes_min: disk_available,
        disk_used_ratio_avg: disk_used_ratio,
        disk_used_ratio_max: disk_used_ratio,
        network_rx_bytes_max: network_rx,
        network_tx_bytes_max: network_tx,
        connections_sample_count: i32::from(metrics.connections.is_some()),
        tcp_sockets_latest: metrics
            .connections
            .as_ref()
            .map(|connections| u64_to_i64(connections.tcp)),
        udp_sockets_latest: metrics
            .connections
            .as_ref()
            .map(|connections| u64_to_i64(connections.udp)),
        connections_observed_at: metrics.connections.as_ref().map(|_| observed_at.clone()),
        latest_observed_at: observed_at.clone(),
        updated_at: observed_at,
    });
}

async fn upsert_memory_telemetry_network_rates(
    rates: &Arc<RwLock<Vec<TelemetryNetworkRateView>>>,
    traffic_samples: &Arc<RwLock<Vec<TrafficCounterSampleRecord>>>,
    client_id: &str,
    metrics: &AgentMetrics,
) {
    let bucket_start = bucket_start_unix(metrics.observed_unix).to_string();
    let observed_at = metrics.observed_unix.to_string();
    let bucket_unix = bucket_start_unix(metrics.observed_unix) as i64;
    let epochs_by_interface = traffic_samples
        .read()
        .await
        .iter()
        .filter(|sample| {
            sample.client_id == client_id
                && sample.source_kind == "host"
                && sample.observed_unix == bucket_unix
        })
        .map(|sample| {
            (
                sample.interface.clone(),
                (sample.rx_counter_epoch, sample.tx_counter_epoch),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut rates = rates.write().await;
    for network in metrics
        .networks
        .iter()
        .filter(|network| valid_telemetry_name(&network.interface))
    {
        let Some(&(rx_counter_epoch, tx_counter_epoch)) =
            epochs_by_interface.get(&network.interface)
        else {
            continue;
        };
        let rx_bytes = u64_to_i64(network.rx_bytes);
        let tx_bytes = u64_to_i64(network.tx_bytes);
        if let Some(rate) = rates.iter_mut().find(|rate| {
            rate.client_id == client_id
                && rate.interface == network.interface
                && rate.bucket_secs == TELEMETRY_BUCKET_SECS
                && rate.bucket_start == bucket_start
        }) {
            let current_count = rate.sample_count.max(1);
            rate.sample_count = rate.sample_count.saturating_add(1);
            rate.rx_bytes_avg = weighted_avg_i64(rate.rx_bytes_avg, current_count, rx_bytes);
            rate.tx_bytes_avg = weighted_avg_i64(rate.tx_bytes_avg, current_count, tx_bytes);
            rate.rx_bytes_last = rx_bytes;
            rate.tx_bytes_last = tx_bytes;
            rate.rx_counter_epoch = rx_counter_epoch;
            rate.tx_counter_epoch = tx_counter_epoch;
            rate.latest_observed_at = observed_at.clone();
            rate.rx_bytes_delta = 0;
            rate.tx_bytes_delta = 0;
            rate.rx_bps_avg = 0.0;
            rate.tx_bps_avg = 0.0;
            rate.updated_at = observed_at.clone();
            continue;
        }

        rates.push(TelemetryNetworkRateView {
            client_id: client_id.to_string(),
            interface: network.interface.clone(),
            bucket_start: bucket_start.clone(),
            bucket_secs: TELEMETRY_BUCKET_SECS,
            sample_count: 1,
            rx_bytes_avg: rx_bytes,
            tx_bytes_avg: tx_bytes,
            rx_bytes_last: rx_bytes,
            tx_bytes_last: tx_bytes,
            rx_counter_epoch,
            tx_counter_epoch,
            latest_observed_at: observed_at.clone(),
            rx_bytes_delta: 0,
            tx_bytes_delta: 0,
            rx_bps_avg: 0.0,
            tx_bps_avg: 0.0,
            updated_at: observed_at.clone(),
        });
    }
}

async fn upsert_memory_traffic_counter_samples(
    samples: &Arc<RwLock<Vec<TrafficCounterSampleRecord>>>,
    client_id: &str,
    metrics: &AgentMetrics,
) {
    let bucket_unix = bucket_start_unix(metrics.observed_unix);
    let observed_at = Utc
        .timestamp_opt(bucket_unix as i64, 0)
        .single()
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| bucket_unix.to_string());
    let mut samples = samples.write().await;
    for network in metrics
        .networks
        .iter()
        .filter(|network| valid_telemetry_name(&network.interface))
    {
        upsert_memory_traffic_counter(
            &mut samples,
            TrafficCounterSampleRecord {
                client_id: client_id.to_string(),
                source_kind: "host".to_string(),
                interface: network.interface.clone(),
                observed_at: observed_at.clone(),
                observed_unix: bucket_unix as i64,
                rx_bytes: u64_to_i64(network.rx_bytes),
                tx_bytes: u64_to_i64(network.tx_bytes),
                rx_counter_epoch: 0,
                tx_counter_epoch: 0,
                sample_source: "agent_networks".to_string(),
            },
        );
    }
    for tunnel in metrics.tunnels.iter().filter(|tunnel| valid_tunnel(tunnel)) {
        upsert_memory_traffic_counter(
            &mut samples,
            TrafficCounterSampleRecord {
                client_id: client_id.to_string(),
                source_kind: "tunnel".to_string(),
                interface: tunnel.interface.clone(),
                observed_at: observed_at.clone(),
                observed_unix: bucket_unix as i64,
                rx_bytes: u64_to_i64(tunnel.rx_bytes),
                tx_bytes: u64_to_i64(tunnel.tx_bytes),
                rx_counter_epoch: 0,
                tx_counter_epoch: 0,
                sample_source: tunnel
                    .traffic_source
                    .clone()
                    .unwrap_or_else(|| "runtime_tunnel".to_string()),
            },
        );
    }
}

fn upsert_memory_traffic_counter(
    samples: &mut Vec<TrafficCounterSampleRecord>,
    mut sample: TrafficCounterSampleRecord,
) {
    if let Some(stored) = samples.iter_mut().find(|stored| {
        stored.client_id == sample.client_id
            && stored.source_kind == sample.source_kind
            && stored.interface == sample.interface
            && stored.observed_unix == sample.observed_unix
    }) {
        let source_boundary =
            is_intentional_vnstat_import_boundary(&stored.sample_source, &sample.sample_source);
        sample.rx_counter_epoch = stored.rx_counter_epoch
            + i64::from(sample.rx_bytes < stored.rx_bytes || source_boundary);
        sample.tx_counter_epoch = stored.tx_counter_epoch
            + i64::from(sample.tx_bytes < stored.tx_bytes || source_boundary);
        *stored = sample;
    } else {
        if let Some(previous) = samples
            .iter()
            .filter(|stored| {
                stored.client_id == sample.client_id
                    && stored.source_kind == sample.source_kind
                    && stored.interface == sample.interface
                    && stored.observed_unix < sample.observed_unix
            })
            .max_by_key(|stored| stored.observed_unix)
        {
            let source_boundary = is_intentional_vnstat_import_boundary(
                &previous.sample_source,
                &sample.sample_source,
            );
            sample.rx_counter_epoch = previous.rx_counter_epoch
                + i64::from(sample.rx_bytes < previous.rx_bytes || source_boundary);
            sample.tx_counter_epoch = previous.tx_counter_epoch
                + i64::from(sample.tx_bytes < previous.tx_bytes || source_boundary);
        }
        samples.push(sample);
    }
}

async fn upsert_postgres_telemetry_rollup(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    metrics: &AgentMetrics,
    swap: Option<(i64, i64)>,
) -> Result<()> {
    let (disk_total, disk_available, network_rx, network_tx) = telemetry_totals(metrics);
    let positive_swap = swap.filter(|(total, _)| *total > 0);
    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id,
            bucket_start,
            bucket_secs,
            sample_count,
            cpu_usage_sample_count,
            cpu_usage_sum,
            cpu_usage_avg,
            cpu_usage_max,
            cpu_cores_max,
            cpu_load_1_avg,
            cpu_load_1_sum,
            cpu_load_1_max,
            cpu_load_5_avg,
            cpu_load_5_sum,
            cpu_load_5_max,
            cpu_load_15_avg,
            cpu_load_15_sum,
            cpu_load_15_max,
            memory_total_bytes_max,
            memory_available_bytes_avg,
            memory_available_bytes_sum,
            memory_available_bytes_min,
            memory_used_ratio_avg,
            memory_used_ratio_sum,
            memory_used_ratio_max,
            swap_sample_count,
            swap_total_bytes_max,
            swap_available_bytes_avg,
            swap_available_bytes_sum,
            swap_available_bytes_min,
            swap_used_ratio_avg,
            swap_used_ratio_sum,
            swap_used_ratio_max,
            disk_total_bytes_max,
            disk_available_bytes_avg,
            disk_available_bytes_sum,
            disk_available_bytes_min,
            disk_used_ratio_avg,
            disk_used_ratio_sum,
            disk_used_ratio_max,
            network_rx_bytes_max,
            network_tx_bytes_max,
            connections_sample_count,
            tcp_sockets_latest,
            udp_sockets_latest,
            connections_observed_at,
            latest_observed_at,
            updated_at
        )
        VALUES (
            $1,
            to_timestamp($2::double precision),
            $3,
            1,
            $4,
            COALESCE($5, 0),
            $5,
            $6,
            $7,
            $8,
            $8,
            $8,
            $9,
            $9,
            $9,
            $10,
            $10,
            $10,
            $11,
            $12,
            $12::numeric,
            $12,
            $13,
            $13,
            $13,
            $14,
            $15,
            $16,
            COALESCE($16, 0)::numeric,
            $16,
            $17,
            COALESCE($17, 0),
            $17,
            $18,
            $19,
            $19::numeric,
            $19,
            $20,
            $20,
            $20,
            $21,
            $22,
            $23,
            $24,
            $25,
            CASE WHEN $26::double precision IS NULL
                THEN NULL
                ELSE to_timestamp($26::double precision)
            END,
            to_timestamp($27::double precision),
            now()
        )
        ON CONFLICT (client_id, bucket_secs, bucket_start) DO UPDATE SET
            sample_count = telemetry_rollups.sample_count + EXCLUDED.sample_count,
            cpu_usage_sample_count = telemetry_rollups.cpu_usage_sample_count
                + EXCLUDED.cpu_usage_sample_count,
            cpu_usage_sum = telemetry_rollups.cpu_usage_sum + EXCLUDED.cpu_usage_sum,
            cpu_usage_avg = CASE
                WHEN telemetry_rollups.cpu_usage_sample_count + EXCLUDED.cpu_usage_sample_count = 0
                    THEN NULL
                ELSE (telemetry_rollups.cpu_usage_sum + EXCLUDED.cpu_usage_sum) / (
                    telemetry_rollups.cpu_usage_sample_count
                    + EXCLUDED.cpu_usage_sample_count
                )::double precision
            END,
            cpu_usage_max = CASE
                WHEN telemetry_rollups.cpu_usage_max IS NULL THEN EXCLUDED.cpu_usage_max
                WHEN EXCLUDED.cpu_usage_max IS NULL THEN telemetry_rollups.cpu_usage_max
                ELSE GREATEST(telemetry_rollups.cpu_usage_max, EXCLUDED.cpu_usage_max)
            END,
            cpu_cores_max = GREATEST(telemetry_rollups.cpu_cores_max, EXCLUDED.cpu_cores_max),
            cpu_load_1_sum = telemetry_rollups.cpu_load_1_sum + EXCLUDED.cpu_load_1_sum,
            cpu_load_1_avg = (telemetry_rollups.cpu_load_1_sum + EXCLUDED.cpu_load_1_sum)
                / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::double precision,
            cpu_load_1_max = GREATEST(telemetry_rollups.cpu_load_1_max, EXCLUDED.cpu_load_1_max),
            cpu_load_5_sum = telemetry_rollups.cpu_load_5_sum + EXCLUDED.cpu_load_5_sum,
            cpu_load_5_avg = (telemetry_rollups.cpu_load_5_sum + EXCLUDED.cpu_load_5_sum)
                / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::double precision,
            cpu_load_5_max = GREATEST(telemetry_rollups.cpu_load_5_max, EXCLUDED.cpu_load_5_max),
            cpu_load_15_sum = telemetry_rollups.cpu_load_15_sum + EXCLUDED.cpu_load_15_sum,
            cpu_load_15_avg = (telemetry_rollups.cpu_load_15_sum + EXCLUDED.cpu_load_15_sum)
                / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::double precision,
            cpu_load_15_max = GREATEST(telemetry_rollups.cpu_load_15_max, EXCLUDED.cpu_load_15_max),
            memory_total_bytes_max = GREATEST(
                telemetry_rollups.memory_total_bytes_max,
                EXCLUDED.memory_total_bytes_max
            ),
            memory_available_bytes_sum = telemetry_rollups.memory_available_bytes_sum
                + EXCLUDED.memory_available_bytes_sum,
            memory_available_bytes_avg = round((
                telemetry_rollups.memory_available_bytes_sum
                + EXCLUDED.memory_available_bytes_sum
            ) / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::numeric)::bigint,
            memory_available_bytes_min = LEAST(
                telemetry_rollups.memory_available_bytes_min,
                EXCLUDED.memory_available_bytes_min
            ),
            memory_used_ratio_sum = telemetry_rollups.memory_used_ratio_sum
                + EXCLUDED.memory_used_ratio_sum,
            memory_used_ratio_avg = (
                telemetry_rollups.memory_used_ratio_sum + EXCLUDED.memory_used_ratio_sum
            ) / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::double precision,
            memory_used_ratio_max = GREATEST(
                telemetry_rollups.memory_used_ratio_max,
                EXCLUDED.memory_used_ratio_max
            ),
            swap_sample_count = telemetry_rollups.swap_sample_count
                + EXCLUDED.swap_sample_count,
            swap_total_bytes_max = CASE
                WHEN telemetry_rollups.swap_total_bytes_max IS NULL
                    THEN EXCLUDED.swap_total_bytes_max
                WHEN EXCLUDED.swap_total_bytes_max IS NULL
                    THEN telemetry_rollups.swap_total_bytes_max
                ELSE GREATEST(
                    telemetry_rollups.swap_total_bytes_max,
                    EXCLUDED.swap_total_bytes_max
                )
            END,
            swap_available_bytes_sum = telemetry_rollups.swap_available_bytes_sum
                + EXCLUDED.swap_available_bytes_sum,
            swap_available_bytes_avg = CASE
                WHEN telemetry_rollups.swap_sample_count + EXCLUDED.swap_sample_count = 0
                    THEN CASE
                        WHEN telemetry_rollups.swap_total_bytes_max IS NULL
                            AND EXCLUDED.swap_total_bytes_max IS NULL
                            THEN NULL
                        ELSE 0
                    END
                ELSE round((
                    telemetry_rollups.swap_available_bytes_sum
                    + EXCLUDED.swap_available_bytes_sum
                ) / (
                    telemetry_rollups.swap_sample_count + EXCLUDED.swap_sample_count
                )::numeric)::bigint
            END,
            swap_available_bytes_min = CASE
                WHEN telemetry_rollups.swap_sample_count + EXCLUDED.swap_sample_count = 0
                    THEN CASE
                        WHEN telemetry_rollups.swap_total_bytes_max IS NULL
                            AND EXCLUDED.swap_total_bytes_max IS NULL
                            THEN NULL
                        ELSE 0
                    END
                WHEN telemetry_rollups.swap_sample_count = 0
                    THEN EXCLUDED.swap_available_bytes_min
                WHEN EXCLUDED.swap_sample_count = 0
                    THEN telemetry_rollups.swap_available_bytes_min
                ELSE LEAST(
                    telemetry_rollups.swap_available_bytes_min,
                    EXCLUDED.swap_available_bytes_min
                )
            END,
            swap_used_ratio_sum = telemetry_rollups.swap_used_ratio_sum
                + EXCLUDED.swap_used_ratio_sum,
            swap_used_ratio_avg = CASE
                WHEN telemetry_rollups.swap_sample_count + EXCLUDED.swap_sample_count = 0
                    THEN NULL
                ELSE (telemetry_rollups.swap_used_ratio_sum + EXCLUDED.swap_used_ratio_sum) / (
                    telemetry_rollups.swap_sample_count + EXCLUDED.swap_sample_count
                )::double precision
            END,
            swap_used_ratio_max = CASE
                WHEN telemetry_rollups.swap_used_ratio_max IS NULL
                    THEN EXCLUDED.swap_used_ratio_max
                WHEN EXCLUDED.swap_used_ratio_max IS NULL
                    THEN telemetry_rollups.swap_used_ratio_max
                ELSE GREATEST(
                    telemetry_rollups.swap_used_ratio_max,
                    EXCLUDED.swap_used_ratio_max
                )
            END,
            disk_total_bytes_max = GREATEST(
                telemetry_rollups.disk_total_bytes_max,
                EXCLUDED.disk_total_bytes_max
            ),
            disk_available_bytes_sum = telemetry_rollups.disk_available_bytes_sum
                + EXCLUDED.disk_available_bytes_sum,
            disk_available_bytes_avg = round((
                telemetry_rollups.disk_available_bytes_sum
                + EXCLUDED.disk_available_bytes_sum
            ) / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::numeric)::bigint,
            disk_available_bytes_min = LEAST(
                telemetry_rollups.disk_available_bytes_min,
                EXCLUDED.disk_available_bytes_min
            ),
            disk_used_ratio_sum = telemetry_rollups.disk_used_ratio_sum
                + EXCLUDED.disk_used_ratio_sum,
            disk_used_ratio_avg = (
                telemetry_rollups.disk_used_ratio_sum + EXCLUDED.disk_used_ratio_sum
            ) / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::double precision,
            disk_used_ratio_max = GREATEST(
                telemetry_rollups.disk_used_ratio_max,
                EXCLUDED.disk_used_ratio_max
            ),
            network_rx_bytes_max = GREATEST(
                telemetry_rollups.network_rx_bytes_max,
                EXCLUDED.network_rx_bytes_max
            ),
            network_tx_bytes_max = GREATEST(
                telemetry_rollups.network_tx_bytes_max,
                EXCLUDED.network_tx_bytes_max
            ),
            connections_sample_count = telemetry_rollups.connections_sample_count
                + EXCLUDED.connections_sample_count,
            tcp_sockets_latest = CASE
                WHEN EXCLUDED.connections_observed_at IS NULL
                    THEN telemetry_rollups.tcp_sockets_latest
                WHEN telemetry_rollups.connections_observed_at IS NULL
                    OR EXCLUDED.connections_observed_at >= telemetry_rollups.connections_observed_at
                    THEN EXCLUDED.tcp_sockets_latest
                ELSE telemetry_rollups.tcp_sockets_latest
            END,
            udp_sockets_latest = CASE
                WHEN EXCLUDED.connections_observed_at IS NULL
                    THEN telemetry_rollups.udp_sockets_latest
                WHEN telemetry_rollups.connections_observed_at IS NULL
                    OR EXCLUDED.connections_observed_at >= telemetry_rollups.connections_observed_at
                    THEN EXCLUDED.udp_sockets_latest
                ELSE telemetry_rollups.udp_sockets_latest
            END,
            connections_observed_at = CASE
                WHEN telemetry_rollups.connections_observed_at IS NULL
                    THEN EXCLUDED.connections_observed_at
                WHEN EXCLUDED.connections_observed_at IS NULL
                    THEN telemetry_rollups.connections_observed_at
                ELSE GREATEST(
                    telemetry_rollups.connections_observed_at,
                    EXCLUDED.connections_observed_at
                )
            END,
            latest_observed_at = GREATEST(
                telemetry_rollups.latest_observed_at,
                EXCLUDED.latest_observed_at
            ),
            updated_at = now()
        "#,
    )
    .bind(client_id)
    .bind(bucket_start_unix(metrics.observed_unix) as f64)
    .bind(TELEMETRY_BUCKET_SECS)
    .bind(i32::from(metrics.cpu.utilization_ratio.is_some()))
    .bind(metrics.cpu.utilization_ratio)
    .bind(metrics.cpu.utilization_ratio)
    .bind(i32::from(metrics.cpu.cores))
    .bind(metrics.cpu.load.one)
    .bind(metrics.cpu.load.five)
    .bind(metrics.cpu.load.fifteen)
    .bind(u64_to_i64(metrics.memory.total_bytes))
    .bind(u64_to_i64(metrics.memory.available_bytes))
    .bind(resource_used_ratio_or_zero(
        u64_to_i64(metrics.memory.total_bytes),
        u64_to_i64(metrics.memory.available_bytes),
    ))
    .bind(i32::from(positive_swap.is_some()))
    .bind(swap.map(|(total, _)| total))
    .bind(swap.map(|(_, available)| available))
    .bind(positive_swap.map(|(total, available)| resource_used_ratio(total, available)))
    .bind(disk_total)
    .bind(disk_available)
    .bind(resource_used_ratio_or_zero(disk_total, disk_available))
    .bind(network_rx)
    .bind(network_tx)
    .bind(i32::from(metrics.connections.is_some()))
    .bind(
        metrics
            .connections
            .as_ref()
            .map(|connections| u64_to_i64(connections.tcp)),
    )
    .bind(
        metrics
            .connections
            .as_ref()
            .map(|connections| u64_to_i64(connections.udp)),
    )
    .bind(
        metrics
            .connections
            .as_ref()
            .map(|_| metrics.observed_unix as f64),
    )
    .bind(metrics.observed_unix as f64)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        DELETE FROM telemetry_resource_latest latest
        USING telemetry_rollups source
        WHERE source.client_id = $1
          AND source.bucket_secs = $2
          AND source.bucket_start = to_timestamp($3::double precision)
          AND latest.client_id = source.client_id
          AND latest.latest_observed_at <= source.latest_observed_at
        "#,
    )
    .bind(client_id)
    .bind(TELEMETRY_BUCKET_SECS)
    .bind(bucket_start_unix(metrics.observed_unix) as f64)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO telemetry_resource_latest
        SELECT source.* FROM telemetry_rollups source
        WHERE source.client_id = $1
          AND source.bucket_secs = $2
          AND source.bucket_start = to_timestamp($3::double precision)
        ON CONFLICT (client_id) DO NOTHING
        "#,
    )
    .bind(client_id)
    .bind(TELEMETRY_BUCKET_SECS)
    .bind(bucket_start_unix(metrics.observed_unix) as f64)
    .execute(&mut **tx)
    .await?;
    Ok(())
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

async fn upsert_postgres_telemetry_network_rates(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    metrics: &AgentMetrics,
) -> Result<()> {
    for network in metrics
        .networks
        .iter()
        .filter(|network| valid_telemetry_name(&network.interface))
    {
        sqlx::query(
            r#"
            INSERT INTO telemetry_network_rates (
                client_id,
                interface,
                bucket_start,
                bucket_secs,
                sample_count,
                rx_bytes_sum,
                tx_bytes_sum,
                rx_bytes_avg,
                tx_bytes_avg,
                rx_bytes_last,
                tx_bytes_last,
                rx_counter_epoch,
                tx_counter_epoch,
                latest_observed_at,
                updated_at
            )
            SELECT
                $1,
                $2,
                to_timestamp($3::double precision),
                $4,
                1,
                $5::numeric,
                $6::numeric,
                $5,
                $6,
                $5,
                $6,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                to_timestamp($7::double precision),
                now()
            FROM traffic_counter_samples sample
            WHERE sample.client_id = $1
              AND sample.source_kind = 'host'
              AND sample.interface = $2
              AND sample.observed_at = to_timestamp($3::double precision)
            ON CONFLICT (client_id, interface, bucket_secs, bucket_start) DO UPDATE SET
                sample_count = telemetry_network_rates.sample_count + EXCLUDED.sample_count,
                rx_bytes_sum = telemetry_network_rates.rx_bytes_sum + EXCLUDED.rx_bytes_sum,
                tx_bytes_sum = telemetry_network_rates.tx_bytes_sum + EXCLUDED.tx_bytes_sum,
                rx_bytes_avg = round((telemetry_network_rates.rx_bytes_sum
                    + EXCLUDED.rx_bytes_sum)
                    / (telemetry_network_rates.sample_count + EXCLUDED.sample_count)::numeric)::bigint,
                tx_bytes_avg = round((telemetry_network_rates.tx_bytes_sum
                    + EXCLUDED.tx_bytes_sum)
                    / (telemetry_network_rates.sample_count + EXCLUDED.sample_count)::numeric)::bigint,
                rx_bytes_last = EXCLUDED.rx_bytes_last,
                tx_bytes_last = EXCLUDED.tx_bytes_last,
                rx_counter_epoch = EXCLUDED.rx_counter_epoch,
                tx_counter_epoch = EXCLUDED.tx_counter_epoch,
                latest_observed_at = GREATEST(
                    telemetry_network_rates.latest_observed_at,
                    EXCLUDED.latest_observed_at
                ),
                updated_at = now()
            "#,
        )
        .bind(client_id)
        .bind(&network.interface)
        .bind(bucket_start_unix(metrics.observed_unix) as f64)
        .bind(TELEMETRY_BUCKET_SECS)
        .bind(u64_to_i64(network.rx_bytes))
        .bind(u64_to_i64(network.tx_bytes))
        .bind(metrics.observed_unix as f64)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn upsert_postgres_traffic_counter_samples(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    metrics: &AgentMetrics,
) -> Result<()> {
    for network in metrics
        .networks
        .iter()
        .filter(|network| valid_telemetry_name(&network.interface))
    {
        insert_traffic_counter_sample(
            tx,
            client_id,
            "host",
            &network.interface,
            metrics.observed_unix,
            u64_to_i64(network.rx_bytes),
            u64_to_i64(network.tx_bytes),
            "agent_networks",
        )
        .await?;
    }
    for tunnel in metrics.tunnels.iter().filter(|tunnel| valid_tunnel(tunnel)) {
        let sample_source = tunnel.traffic_source.as_deref().unwrap_or("runtime_tunnel");
        insert_traffic_counter_sample(
            tx,
            client_id,
            "tunnel",
            &tunnel.interface,
            metrics.observed_unix,
            u64_to_i64(tunnel.rx_bytes),
            u64_to_i64(tunnel.tx_bytes),
            sample_source,
        )
        .await?;
    }
    Ok(())
}

async fn insert_traffic_counter_sample(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    source_kind: &str,
    interface: &str,
    observed_unix: u64,
    rx_bytes: i64,
    tx_bytes: i64,
    sample_source: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        WITH previous AS (
            SELECT rx_counter_epoch, tx_counter_epoch, rx_bytes, tx_bytes, sample_source
            FROM traffic_counter_samples
            WHERE client_id = $1
              AND source_kind = $2
              AND interface = $3
              AND observed_at <= date_trunc(
                    'minute', to_timestamp($4::double precision)
              )
            ORDER BY observed_at DESC
            LIMIT 1
        )
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at, rx_bytes, tx_bytes,
            rx_counter_epoch, tx_counter_epoch, sample_source
        )
        SELECT
            $1, $2, $3,
            date_trunc('minute', to_timestamp($4::double precision)),
            $5,
            $6,
            COALESCE(previous.rx_counter_epoch, 0)
                + CASE
                    WHEN $5 < previous.rx_bytes THEN 1
                    WHEN previous.sample_source LIKE 'vnstat_import:%'
                     AND $7 NOT LIKE 'vnstat_import:%' THEN 1
                    ELSE 0
                  END,
            COALESCE(previous.tx_counter_epoch, 0)
                + CASE
                    WHEN $6 < previous.tx_bytes THEN 1
                    WHEN previous.sample_source LIKE 'vnstat_import:%'
                     AND $7 NOT LIKE 'vnstat_import:%' THEN 1
                    ELSE 0
                  END,
            $7
        FROM (SELECT 1) seed
        LEFT JOIN previous ON TRUE
        ON CONFLICT (client_id, source_kind, interface, observed_at) DO UPDATE SET
            rx_bytes = EXCLUDED.rx_bytes,
            tx_bytes = EXCLUDED.tx_bytes,
            rx_counter_epoch = CASE
                WHEN EXCLUDED.rx_bytes < traffic_counter_samples.rx_bytes
                  OR (
                    traffic_counter_samples.sample_source LIKE 'vnstat_import:%'
                    AND EXCLUDED.sample_source NOT LIKE 'vnstat_import:%'
                  )
                THEN traffic_counter_samples.rx_counter_epoch + 1
                ELSE GREATEST(
                    traffic_counter_samples.rx_counter_epoch,
                    EXCLUDED.rx_counter_epoch
                )
            END,
            tx_counter_epoch = CASE
                WHEN EXCLUDED.tx_bytes < traffic_counter_samples.tx_bytes
                  OR (
                    traffic_counter_samples.sample_source LIKE 'vnstat_import:%'
                    AND EXCLUDED.sample_source NOT LIKE 'vnstat_import:%'
                  )
                THEN traffic_counter_samples.tx_counter_epoch + 1
                ELSE GREATEST(
                    traffic_counter_samples.tx_counter_epoch,
                    EXCLUDED.tx_counter_epoch
                )
            END,
            sample_source = EXCLUDED.sample_source,
            inbound_promoted = FALSE
        "#,
    )
    .bind(client_id)
    .bind(source_kind)
    .bind(interface)
    .bind(observed_unix as f64)
    .bind(rx_bytes)
    .bind(tx_bytes)
    .bind(sample_source)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_postgres_telemetry_tunnels(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    metrics: &AgentMetrics,
) -> Result<()> {
    sqlx::query("DELETE FROM telemetry_tunnels WHERE client_id = $1")
        .bind(client_id)
        .execute(&mut **tx)
        .await?;

    for tunnel in metrics.tunnels.iter().filter(|tunnel| valid_tunnel(tunnel)) {
        let adapter_health = tunnel
            .adapter_health
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        sqlx::query(
            r#"
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
                traffic_source,
                traffic_status,
                traffic_reason,
                traffic_checked_unix,
                telemetry_plan_id,
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
                updated_at
            )
            VALUES (
                $1,
                to_timestamp($2::double precision),
                $3,
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
                $19,
                $20,
                $21,
                $22,
                $23,
                $24,
                $25,
                $26,
                $27,
                $28,
                $29,
                $30,
                $31,
                $32,
                $33,
                now()
            )
            "#,
        )
        .bind(client_id)
        .bind(metrics.observed_unix as f64)
        .bind(&tunnel.interface)
        .bind(&tunnel.kind)
        .bind(&tunnel.ownership_mode)
        .bind(&tunnel.mutation_policy)
        .bind(&tunnel.source)
        .bind(&tunnel.operstate)
        .bind(tunnel.mtu.map(u64_to_i64))
        .bind(tunnel.link_type)
        .bind(&tunnel.address)
        .bind(u64_to_i64(tunnel.rx_bytes))
        .bind(u64_to_i64(tunnel.tx_bytes))
        .bind(&tunnel.traffic_source)
        .bind(&tunnel.traffic_status)
        .bind(&tunnel.traffic_reason)
        .bind(tunnel.traffic_checked_unix.map(u64_to_i64))
        .bind(&tunnel.plan_id)
        .bind(&tunnel.plan_name)
        .bind(&tunnel.plan_runtime_manager)
        .bind(&tunnel.endpoint_side)
        .bind(&tunnel.peer_client_id)
        .bind(adapter_health)
        .bind(tunnel.latency_monitoring_enabled)
        .bind(&tunnel.latency_status)
        .bind(&tunnel.latency_reason)
        .bind(&tunnel.latency_primary_family)
        .bind(&tunnel.latency_target)
        .bind(tunnel.latency_checked_unix.map(u64_to_i64))
        .bind(tunnel.latency_avg_ms)
        .bind(tunnel.packet_loss_ratio)
        .bind(tunnel.latency_healthy_windows.map(i32::from))
        .bind(tunnel.latency_missed_windows.map(i32::from))
        .execute(&mut **tx)
        .await?;
    }
    reconcile_postgres_automatic_observation_series_for_client(tx, client_id).await?;
    Ok(())
}

fn telemetry_tunnel_view(
    client_id: &str,
    observed_unix: u64,
    tunnel: &RuntimeTunnelStat,
) -> Option<TelemetryTunnelView> {
    if !valid_tunnel(tunnel) {
        return None;
    }
    Some(TelemetryTunnelView {
        client_id: client_id.to_string(),
        observed_at: observed_unix.to_string(),
        interface: tunnel.interface.clone(),
        kind: tunnel.kind.clone(),
        ownership_mode: tunnel.ownership_mode.clone(),
        mutation_policy: tunnel.mutation_policy.clone(),
        plan_id: tunnel
            .plan_id
            .as_deref()
            .and_then(|value| Uuid::parse_str(value).ok()),
        plan_name: tunnel.plan_name.clone(),
        plan_runtime_manager: tunnel.plan_runtime_manager.clone(),
        endpoint_side: tunnel.endpoint_side.clone(),
        peer_client_id: tunnel.peer_client_id.clone(),
        source: tunnel.source.clone(),
        operstate: tunnel.operstate.clone(),
        mtu: tunnel.mtu.map(u64_to_i64),
        link_type: tunnel.link_type,
        address: tunnel.address.clone(),
        rx_bytes: u64_to_i64(tunnel.rx_bytes),
        tx_bytes: u64_to_i64(tunnel.tx_bytes),
        traffic_source: tunnel.traffic_source.clone(),
        traffic_status: tunnel.traffic_status.clone(),
        traffic_reason: tunnel.traffic_reason.clone(),
        traffic_checked_unix: tunnel.traffic_checked_unix.map(u64_to_i64),
        adapter_health: tunnel.adapter_health.as_ref().map(adapter_health_view),
        latency_monitoring_enabled: tunnel.latency_monitoring_enabled,
        latency_status: tunnel.latency_status.clone(),
        latency_reason: tunnel.latency_reason.clone(),
        latency_primary_family: tunnel.latency_primary_family.clone(),
        latency_target: tunnel.latency_target.clone(),
        latency_checked_unix: tunnel.latency_checked_unix.map(u64_to_i64),
        latency_avg_ms: tunnel.latency_avg_ms,
        packet_loss_ratio: tunnel.packet_loss_ratio,
        latency_healthy_windows: tunnel.latency_healthy_windows.map(i32::from),
        latency_missed_windows: tunnel.latency_missed_windows.map(i32::from),
    })
}

fn agent_hello_session_event(event: &GatewayAgentHelloIngest) -> GatewaySessionLifecycleIngest {
    GatewaySessionLifecycleIngest {
        gateway_id: event.gateway_id.clone(),
        client_id: event.hello.client_id.clone(),
        session_id: event.gateway_session_id,
        noise_public_key_hex: Some(event.noise_public_key_hex.clone()),
        remote_ip: event.remote_ip.clone(),
        agent_version: Some(event.hello.agent_version.clone()),
        reason: None,
    }
}

fn adapter_health_view(
    health: &RuntimeTunnelAdapterHealthStat,
) -> TelemetryTunnelAdapterHealthView {
    TelemetryTunnelAdapterHealthView {
        status: health.status.clone(),
        checked_unix: u64_to_i64(health.checked_unix),
        configured: health.configured,
        success: health.success,
        exit_code: health.exit_code,
        reason: health.reason.clone(),
        duration_ms: u64_to_i64(health.duration_ms),
        command_sha256_hex: health.command_sha256_hex.clone(),
        timed_out: health.timed_out,
        output_truncated: health.output_truncated,
        stdout_sha256_hex: health.stdout_sha256_hex.clone(),
        stderr_sha256_hex: health.stderr_sha256_hex.clone(),
    }
}

fn telemetry_totals(metrics: &AgentMetrics) -> (i64, i64, i64, i64) {
    let disk_total = sum_u64(metrics.disks.iter().map(|disk| disk.total_bytes));
    let disk_available = sum_u64(metrics.disks.iter().map(|disk| disk.available_bytes));
    let network_rx = sum_u64(metrics.networks.iter().map(|network| network.rx_bytes));
    let network_tx = sum_u64(metrics.networks.iter().map(|network| network.tx_bytes));
    (disk_total, disk_available, network_rx, network_tx)
}

fn weighted_avg_f64(current_avg: f64, current_count: i32, next_value: f64) -> f64 {
    let current_count = current_count.max(1) as f64;
    ((current_avg * current_count) + next_value) / (current_count + 1.0)
}

fn weighted_avg_i64(current_avg: i64, current_count: i32, next_value: i64) -> i64 {
    let current_count = i128::from(current_count.max(1));
    let numerator = i128::from(current_avg) * current_count + i128::from(next_value);
    let denominator = current_count + 1;
    ((numerator + denominator / 2) / denominator).clamp(i128::from(i64::MIN), i128::from(i64::MAX))
        as i64
}

fn resource_used_ratio(total: i64, available: i64) -> f64 {
    debug_assert!(total > 0);
    (total.saturating_sub(available).max(0) as f64 / total.max(1) as f64).clamp(0.0, 1.0)
}

fn resource_used_ratio_or_zero(total: i64, available: i64) -> f64 {
    if total == 0 {
        0.0
    } else {
        resource_used_ratio(total, available)
    }
}

fn bucket_start_unix(observed_unix: u64) -> u64 {
    observed_unix / TELEMETRY_BUCKET_SECS as u64 * TELEMETRY_BUCKET_SECS as u64
}

fn parse_unix(value: &str) -> u64 {
    value.parse::<u64>().unwrap_or(0)
}

fn valid_tunnel(tunnel: &RuntimeTunnelStat) -> bool {
    valid_telemetry_name(&tunnel.interface)
        && valid_telemetry_name(&tunnel.kind)
        && tunnel
            .plan_id
            .as_deref()
            .is_some_and(|value| Uuid::parse_str(value).is_ok())
        && tunnel
            .plan_name
            .as_deref()
            .is_some_and(valid_telemetry_name)
        && matches!(tunnel.endpoint_side.as_deref(), Some("left" | "right"))
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
    crate::repository_webhook_rules::ensure_webhook_event_partition_in_tx(tx, occurred_at).await?;
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

#[cfg(test)]
pub(crate) async fn upsert_memory_agent(agents: &Arc<RwLock<Vec<AgentView>>>, hello: &AgentHello) {
    upsert_memory_agent_with_remote_ip(agents, hello, None).await;
}

pub(crate) async fn upsert_memory_agent_with_remote_ip(
    agents: &Arc<RwLock<Vec<AgentView>>>,
    hello: &AgentHello,
    remote_ip: Option<&str>,
) {
    let mut agents = agents.write().await;
    let now = crate::unix_now().to_string();
    if let Some(agent) = agents.iter_mut().find(|agent| agent.id == hello.client_id) {
        if agent.status != "stale"
            || (!hello.agent_version.is_empty()
                && agent.internal_build_number != hello.internal_build_number)
        {
            agent.status = "online".to_string();
            agent.stale_since = None;
            agent.stale_reason = None;
        }
        if agent.registration_ip.is_none() {
            agent.registration_ip = remote_ip.map(str::to_string);
        }
        if let Some(remote_ip) = remote_ip {
            agent.last_ip = Some(remote_ip.to_string());
        }
        agent.last_seen_at = Some(now);
        if !hello.agent_version.is_empty() {
            agent.internal_build_number = hello.internal_build_number.max(1);
        }
        agent.process_incarnation_id = Some(hello.process_incarnation_id);
        agent.arch = (!hello.arch.trim().is_empty()).then(|| hello.arch.clone());
        agent.capabilities = hello.capabilities.clone();
        return;
    }
    agents.push(AgentView {
        id: hello.client_id.clone(),
        display_name: hello.client_id.clone(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: remote_ip.map(str::to_string),
        last_ip: remote_ip.map(str::to_string),
        last_seen_at: Some(now),
        arch: (!hello.arch.trim().is_empty()).then(|| hello.arch.clone()),
        internal_build_number: hello.internal_build_number.max(1),
        process_incarnation_id: Some(hello.process_incarnation_id),
        stale_since: None,
        stale_reason: None,
        capabilities: hello.capabilities.clone(),
    });
}

async fn touch_memory_agent_from_telemetry(
    agents: &Arc<RwLock<Vec<AgentView>>>,
    client_id: &str,
    remote_ip: Option<&str>,
) {
    let mut agents = agents.write().await;
    let Some(agent) = agents.iter_mut().find(|agent| agent.id == client_id) else {
        return;
    };
    if agent.status != "stale" {
        agent.status = "online".to_string();
        agent.stale_since = None;
        agent.stale_reason = None;
    }
    if agent.registration_ip.is_none() {
        agent.registration_ip = remote_ip.map(str::to_string);
    }
    if let Some(remote_ip) = remote_ip {
        agent.last_ip = Some(remote_ip.to_string());
    }
    agent.last_seen_at = Some(crate::unix_now().to_string());
}
