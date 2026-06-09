use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::{types::Json as SqlJson, Postgres, Row, Transaction};
use tokio::sync::RwLock;
use tracing::debug;
use uuid::Uuid;
use vpsman_common::{
    AgentHello, AgentMetrics, GatewayAgentHelloIngest, GatewayTelemetryIngest,
    RuntimeTunnelAdapterHealthStat, RuntimeTunnelStat,
};

use crate::model::{
    AgentView, TelemetryNetworkRateView, TelemetryRollupView, TelemetryTunnelAdapterHealthView,
    TelemetryTunnelView,
};
use crate::model_webhook_rules::WebhookEventCandidate;
use crate::repository::Repository;
use crate::security::constant_time_eq;

const TELEMETRY_BUCKET_SECS: i32 = 60;

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
        if self.is_client_key_revoked(client_id, &provided).await? {
            return Ok(false);
        }
        match self {
            Self::Memory(memory) => Ok(memory
                .client_public_keys
                .read()
                .await
                .get(client_id)
                .is_some_and(|expected| constant_time_eq(expected, &provided))
                && !memory.hidden_clients.read().await.contains(client_id)),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT public_key
                    FROM clients
                    WHERE id = $1 AND hidden_at IS NULL
                    "#,
                )
                .bind(client_id)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(false);
                };
                let expected: Vec<u8> = row.try_get("public_key")?;
                Ok(constant_time_eq(&expected, &provided))
            }
        }
    }

    pub(crate) async fn upsert_agent_hello(&self, event: &GatewayAgentHelloIngest) -> Result<()> {
        let update_heartbeat = event.hello.update_heartbeat.clone();
        let mut accepted_hello = true;
        match self {
            Self::Memory(memory) => {
                if !memory
                    .hidden_clients
                    .read()
                    .await
                    .contains(&event.hello.client_id)
                {
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
                    if let Some((prior_status, prior_build, stale_reason)) = prior {
                        if prior_status == "stale"
                            && !event.hello.agent_version.is_empty()
                            && prior_build != event.hello.internal_build_number
                        {
                            let metadata = serde_json::json!({
                                "from_status": "stale",
                                "to_status": "online",
                                "reason": "agent_reconnected_with_changed_internal_build",
                                "stale_reason": stale_reason,
                                "previous_internal_build_number": prior_build,
                                "internal_build_number": event.hello.internal_build_number,
                            });
                            memory
                                .audits
                                .write()
                                .await
                                .push(crate::model::AuditLogView {
                                    id: Uuid::new_v4(),
                                    actor_id: None,
                                    action: "agent.status_online".to_string(),
                                    target: format!("client:{}", event.hello.client_id),
                                    command_hash: None,
                                    metadata: metadata.clone(),
                                    created_at: crate::unix_now().to_string(),
                                });
                            self.record_client_status_webhook_event(
                                &event.hello.client_id,
                                Some("stale"),
                                "online",
                                "agent_reconnected_with_changed_internal_build",
                                metadata,
                            )
                            .await?;
                        } else if prior_status == "never" {
                            let metadata = serde_json::json!({
                                "from_status": "never",
                                "to_status": "online",
                                "reason": "agent_first_connection",
                            });
                            memory
                                .audits
                                .write()
                                .await
                                .push(crate::model::AuditLogView {
                                    id: Uuid::new_v4(),
                                    actor_id: None,
                                    action: "agent.status_online".to_string(),
                                    target: format!("client:{}", event.hello.client_id),
                                    command_hash: None,
                                    metadata: metadata.clone(),
                                    created_at: crate::unix_now().to_string(),
                                });
                            self.record_client_status_webhook_event(
                                &event.hello.client_id,
                                Some("never"),
                                "online",
                                "agent_first_connection",
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
                let public_key = match event.noise_public_key_hex.as_deref() {
                    Some(value) => hex::decode(value).with_context(|| {
                        format!("invalid noise public key hex for {}", event.hello.client_id)
                    })?,
                    None => Vec::new(),
                };
                let mut tx = pool.begin().await?;
                let prior = sqlx::query(
                    r#"
                    SELECT status, internal_build_number, stale_build_number
                    FROM clients
                    WHERE id = $1 AND hidden_at IS NULL
                    "#,
                )
                .bind(&event.hello.client_id)
                .fetch_optional(&mut *tx)
                .await?;
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
                let clears_stale = prior_status.as_deref() == Some("stale")
                    && event.hello.internal_build_number as i64 != stale_build;
                let result = sqlx::query(
                    r#"
                    INSERT INTO clients (
                        id, display_name, public_key, status, agent_version,
                        internal_build_number, os_release, arch, capabilities, registration_ip,
                        last_ip, last_seen_at
                    )
                    VALUES ($1, $2, $3, 'online', $4, $5, $6, $7, $8, $9::inet, $9::inet, now())
                    ON CONFLICT (id) DO UPDATE SET
                        public_key = CASE
                            WHEN octet_length(EXCLUDED.public_key) > 0 THEN EXCLUDED.public_key
                            ELSE clients.public_key
                        END,
                        status = CASE
                            WHEN clients.status = 'stale'
                             AND EXCLUDED.internal_build_number = COALESCE(clients.stale_build_number, clients.internal_build_number)
                                THEN 'stale'
                            ELSE 'online'
                        END,
                        agent_version = EXCLUDED.agent_version,
                        internal_build_number = EXCLUDED.internal_build_number,
                        os_release = EXCLUDED.os_release,
                        arch = EXCLUDED.arch,
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
                .bind(public_key)
                .bind(&event.hello.agent_version)
                .bind(event.hello.internal_build_number as i64)
                .bind(&event.hello.os_release)
                .bind(&event.hello.arch)
                .bind(sqlx::types::Json(&event.hello.capabilities))
                .bind(event.remote_ip.as_deref())
                .execute(&mut *tx)
                .await?;
                accepted_hello = result.rows_affected() > 0;
                if accepted_hello && clears_stale {
                    record_client_status_transition_in_tx(
                        &mut tx,
                        &event.hello.client_id,
                        Some("stale"),
                        "online",
                        "agent_reconnected_with_changed_internal_build",
                        serde_json::json!({
                            "old_internal_build_number": prior_build,
                            "stale_build_number": stale_build,
                            "new_internal_build_number": event.hello.internal_build_number,
                            "gateway_id": &event.gateway_id,
                        }),
                    )
                    .await?;
                }
                if accepted_hello && prior_status.as_deref() == Some("never") {
                    record_client_status_transition_in_tx(
                        &mut tx,
                        &event.hello.client_id,
                        Some("never"),
                        "online",
                        "agent_first_connection",
                        serde_json::json!({
                            "gateway_id": &event.gateway_id,
                        }),
                    )
                    .await?;
                }

                tx.commit().await?;
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
        Ok(())
    }

    pub(crate) async fn record_telemetry(&self, event: &GatewayTelemetryIngest) -> Result<()> {
        let record_result: Result<()> = match self {
            Self::Memory(memory) => {
                if memory
                    .hidden_clients
                    .read()
                    .await
                    .contains(&event.telemetry.client_id)
                {
                    return Ok(());
                }
                let hello = AgentHello {
                    client_id: event.telemetry.client_id.clone(),
                    agent_version: String::new(),
                    internal_build_number: 1,
                    os_release: String::new(),
                    arch: String::new(),
                    update_heartbeat: None,
                    capabilities: Default::default(),
                };
                upsert_memory_agent_with_remote_ip(
                    &memory.agents,
                    &hello,
                    event.remote_ip.as_deref(),
                )
                .await;
                upsert_memory_telemetry_rollup(
                    &memory.telemetry_rollups,
                    &event.telemetry.client_id,
                    &event.telemetry.metrics,
                )
                .await;
                upsert_memory_telemetry_network_rates(
                    &memory.telemetry_network_rates,
                    &event.telemetry.client_id,
                    &event.telemetry.metrics,
                )
                .await;
                let mut tunnels = memory.telemetry_tunnels.write().await;
                tunnels.retain(|record| record.client_id != event.telemetry.client_id);
                tunnels.extend(event.telemetry.metrics.tunnels.iter().filter_map(|tunnel| {
                    telemetry_tunnel_view(
                        &event.telemetry.client_id,
                        event.telemetry.metrics.observed_unix,
                        tunnel,
                    )
                }));
                Ok(())
            }
            Self::Postgres(pool) => {
                let metrics = &event.telemetry.metrics;
                let mut tx = pool.begin().await?;
                let deleted: bool = sqlx::query_scalar(
                    r#"
                    SELECT COALESCE(
                        (SELECT hidden_at IS NOT NULL FROM clients WHERE id = $1),
                        false
                    )
                    "#,
                )
                .bind(&event.telemetry.client_id)
                .fetch_one(&mut *tx)
                .await?;
                if deleted {
                    tx.commit().await?;
                    return Ok(());
                }
                upsert_postgres_telemetry_rollup(&mut tx, &event.telemetry.client_id, metrics)
                    .await?;
                upsert_postgres_telemetry_network_rates(
                    &mut tx,
                    &event.telemetry.client_id,
                    metrics,
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
                    "#,
                )
                .bind(&event.telemetry.client_id)
                .bind(event.remote_ip.as_deref())
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(())
            }
        };
        record_result?;
        self.record_telemetry_webhook_event(event).await?;
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
        predicates.sort();
        predicates.dedup();
        let (disk_total, disk_available, network_rx, network_tx) = telemetry_totals(metrics);
        let event_id = format!(
            "telemetry:{}:{}",
            event.telemetry.client_id, metrics.observed_unix
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
                    FROM clients
                    WHERE id = $1 AND hidden_at IS NULL
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

async fn upsert_memory_telemetry_rollup(
    rollups: &Arc<RwLock<Vec<TelemetryRollupView>>>,
    client_id: &str,
    metrics: &AgentMetrics,
) {
    let bucket_start = bucket_start_unix(metrics.observed_unix).to_string();
    let observed_at = metrics.observed_unix.to_string();
    let (disk_total, disk_available, network_rx, network_tx) = telemetry_totals(metrics);
    let mut rollups = rollups.write().await;
    if let Some(rollup) = rollups.iter_mut().find(|rollup| {
        rollup.client_id == client_id
            && rollup.bucket_secs == TELEMETRY_BUCKET_SECS
            && rollup.bucket_start == bucket_start
    }) {
        let current_count = rollup.sample_count.max(1);
        rollup.sample_count = rollup.sample_count.saturating_add(1);
        rollup.cpu_load_1_avg =
            weighted_avg_f64(rollup.cpu_load_1_avg, current_count, metrics.cpu.load.one);
        rollup.cpu_load_1_max = rollup.cpu_load_1_max.max(metrics.cpu.load.one);
        rollup.memory_total_bytes_max = rollup
            .memory_total_bytes_max
            .max(u64_to_i64(metrics.memory.total_bytes));
        rollup.memory_available_bytes_avg = weighted_avg_i64(
            rollup.memory_available_bytes_avg,
            current_count,
            u64_to_i64(metrics.memory.available_bytes),
        );
        rollup.memory_available_bytes_min = rollup
            .memory_available_bytes_min
            .min(u64_to_i64(metrics.memory.available_bytes));
        rollup.disk_total_bytes_max = rollup.disk_total_bytes_max.max(disk_total);
        rollup.disk_available_bytes_avg = weighted_avg_i64(
            rollup.disk_available_bytes_avg,
            current_count,
            disk_available,
        );
        rollup.disk_available_bytes_min = rollup.disk_available_bytes_min.min(disk_available);
        rollup.network_rx_bytes_max = rollup.network_rx_bytes_max.max(network_rx);
        rollup.network_tx_bytes_max = rollup.network_tx_bytes_max.max(network_tx);
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
        cpu_load_1_avg: metrics.cpu.load.one,
        cpu_load_1_max: metrics.cpu.load.one,
        memory_total_bytes_max: u64_to_i64(metrics.memory.total_bytes),
        memory_available_bytes_avg: u64_to_i64(metrics.memory.available_bytes),
        memory_available_bytes_min: u64_to_i64(metrics.memory.available_bytes),
        disk_total_bytes_max: disk_total,
        disk_available_bytes_avg: disk_available,
        disk_available_bytes_min: disk_available,
        network_rx_bytes_max: network_rx,
        network_tx_bytes_max: network_tx,
        latest_observed_at: observed_at.clone(),
        updated_at: observed_at,
    });
}

async fn upsert_memory_telemetry_network_rates(
    rates: &Arc<RwLock<Vec<TelemetryNetworkRateView>>>,
    client_id: &str,
    metrics: &AgentMetrics,
) {
    let bucket_start = bucket_start_unix(metrics.observed_unix).to_string();
    let observed_at = metrics.observed_unix.to_string();
    let mut rates = rates.write().await;
    for network in metrics
        .networks
        .iter()
        .filter(|network| valid_telemetry_name(&network.interface))
    {
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
            rx_bytes_delta: 0,
            tx_bytes_delta: 0,
            rx_bps_avg: 0.0,
            tx_bps_avg: 0.0,
            updated_at: observed_at.clone(),
        });
    }
}

async fn upsert_postgres_telemetry_rollup(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    metrics: &AgentMetrics,
) -> Result<()> {
    let (disk_total, disk_available, network_rx, network_tx) = telemetry_totals(metrics);
    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id,
            bucket_start,
            bucket_secs,
            sample_count,
            cpu_load_1_avg,
            cpu_load_1_max,
            memory_total_bytes_max,
            memory_available_bytes_avg,
            memory_available_bytes_min,
            disk_total_bytes_max,
            disk_available_bytes_avg,
            disk_available_bytes_min,
            network_rx_bytes_max,
            network_tx_bytes_max,
            latest_observed_at,
            updated_at
        )
        VALUES (
            $1,
            to_timestamp($2::double precision),
            $3,
            1,
            $4,
            $4,
            $5,
            $6,
            $6,
            $7,
            $8,
            $8,
            $9,
            $10,
            to_timestamp($11::double precision),
            now()
        )
        ON CONFLICT (client_id, bucket_secs, bucket_start) DO UPDATE SET
            sample_count = telemetry_rollups.sample_count + EXCLUDED.sample_count,
            cpu_load_1_avg = (
                telemetry_rollups.cpu_load_1_avg * telemetry_rollups.sample_count::double precision
                + EXCLUDED.cpu_load_1_avg * EXCLUDED.sample_count::double precision
            ) / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::double precision,
            cpu_load_1_max = GREATEST(telemetry_rollups.cpu_load_1_max, EXCLUDED.cpu_load_1_max),
            memory_total_bytes_max = GREATEST(
                telemetry_rollups.memory_total_bytes_max,
                EXCLUDED.memory_total_bytes_max
            ),
            memory_available_bytes_avg = round((
                telemetry_rollups.memory_available_bytes_avg::numeric * telemetry_rollups.sample_count::numeric
                + EXCLUDED.memory_available_bytes_avg::numeric * EXCLUDED.sample_count::numeric
            ) / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::numeric)::bigint,
            memory_available_bytes_min = LEAST(
                telemetry_rollups.memory_available_bytes_min,
                EXCLUDED.memory_available_bytes_min
            ),
            disk_total_bytes_max = GREATEST(
                telemetry_rollups.disk_total_bytes_max,
                EXCLUDED.disk_total_bytes_max
            ),
            disk_available_bytes_avg = round((
                telemetry_rollups.disk_available_bytes_avg::numeric * telemetry_rollups.sample_count::numeric
                + EXCLUDED.disk_available_bytes_avg::numeric * EXCLUDED.sample_count::numeric
            ) / (telemetry_rollups.sample_count + EXCLUDED.sample_count)::numeric)::bigint,
            disk_available_bytes_min = LEAST(
                telemetry_rollups.disk_available_bytes_min,
                EXCLUDED.disk_available_bytes_min
            ),
            network_rx_bytes_max = GREATEST(
                telemetry_rollups.network_rx_bytes_max,
                EXCLUDED.network_rx_bytes_max
            ),
            network_tx_bytes_max = GREATEST(
                telemetry_rollups.network_tx_bytes_max,
                EXCLUDED.network_tx_bytes_max
            ),
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
    .bind(metrics.cpu.load.one)
    .bind(u64_to_i64(metrics.memory.total_bytes))
    .bind(u64_to_i64(metrics.memory.available_bytes))
    .bind(disk_total)
    .bind(disk_available)
    .bind(network_rx)
    .bind(network_tx)
    .bind(metrics.observed_unix as f64)
    .execute(&mut **tx)
    .await?;
    Ok(())
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
                rx_bytes_avg,
                tx_bytes_avg,
                updated_at
            )
            VALUES (
                $1,
                $2,
                to_timestamp($3::double precision),
                $4,
                1,
                $5,
                $6,
                now()
            )
            ON CONFLICT (client_id, interface, bucket_secs, bucket_start) DO UPDATE SET
                sample_count = telemetry_network_rates.sample_count + EXCLUDED.sample_count,
                rx_bytes_avg = round((
                    telemetry_network_rates.rx_bytes_avg::numeric * telemetry_network_rates.sample_count::numeric
                    + EXCLUDED.rx_bytes_avg::numeric * EXCLUDED.sample_count::numeric
                ) / (telemetry_network_rates.sample_count + EXCLUDED.sample_count)::numeric)::bigint,
                tx_bytes_avg = round((
                    telemetry_network_rates.tx_bytes_avg::numeric * telemetry_network_rates.sample_count::numeric
                    + EXCLUDED.tx_bytes_avg::numeric * EXCLUDED.sample_count::numeric
                ) / (telemetry_network_rates.sample_count + EXCLUDED.sample_count)::numeric)::bigint,
                updated_at = now()
            "#,
        )
        .bind(client_id)
        .bind(&network.interface)
        .bind(bucket_start_unix(metrics.observed_unix) as f64)
        .bind(TELEMETRY_BUCKET_SECS)
        .bind(u64_to_i64(network.rx_bytes))
        .bind(u64_to_i64(network.tx_bytes))
        .execute(&mut **tx)
        .await?;
    }
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
                promotion_required,
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
        .bind(tunnel.promotion_required)
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
        .execute(&mut **tx)
        .await?;
    }
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
        promotion_required: tunnel.promotion_required,
        plan_correlation: if tunnel.plan_id.is_some() || tunnel.plan_name.is_some() {
            "telemetry_reported_plan".to_string()
        } else {
            "unmatched".to_string()
        },
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
    })
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

fn bucket_start_unix(observed_unix: u64) -> u64 {
    observed_unix / TELEMETRY_BUCKET_SECS as u64 * TELEMETRY_BUCKET_SECS as u64
}

fn parse_unix(value: &str) -> u64 {
    value.parse::<u64>().unwrap_or(0)
}

fn valid_tunnel(tunnel: &RuntimeTunnelStat) -> bool {
    valid_telemetry_name(&tunnel.interface) && valid_telemetry_name(&tunnel.kind)
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

async fn record_client_status_transition_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    from_status: Option<&str>,
    to_status: &str,
    reason: &str,
    metadata: serde_json::Value,
) -> Result<()> {
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
        internal_build_number: hello.internal_build_number.max(1),
        stale_since: None,
        stale_reason: None,
        capabilities: hello.capabilities.clone(),
    });
}
