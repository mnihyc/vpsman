use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;
use tracing::debug;
use uuid::Uuid;
use vpsman_common::{AgentHello, GatewayAgentHelloIngest, GatewayTelemetryIngest};

use crate::model::AgentView;
use crate::model::{TelemetryTunnelAdapterHealthView, TelemetryTunnelView};
use crate::repository::Repository;
use crate::security::constant_time_eq;
use sqlx::Row;

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
                .is_some_and(|expected| constant_time_eq(expected, &provided))),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT public_key
                    FROM clients
                    WHERE id = $1
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
        match self {
            Self::Memory(memory) => {
                upsert_memory_agent(&memory.agents, &event.hello).await;
            }
            Self::Postgres(pool) => {
                let public_key = match event.noise_public_key_hex.as_deref() {
                    Some(value) => hex::decode(value).with_context(|| {
                        format!("invalid noise public key hex for {}", event.hello.client_id)
                    })?,
                    None => Vec::new(),
                };
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO clients (
                        id, display_name, public_key, status, agent_version,
                        os_release, arch, capabilities, last_seen_at
                    )
                    VALUES ($1, $2, $3, 'connected', $4, $5, $6, $7, now())
                    ON CONFLICT (id) DO UPDATE SET
                        public_key = CASE
                            WHEN octet_length(EXCLUDED.public_key) > 0 THEN EXCLUDED.public_key
                            ELSE clients.public_key
                        END,
                        status = 'connected',
                        agent_version = EXCLUDED.agent_version,
                        os_release = EXCLUDED.os_release,
                        arch = EXCLUDED.arch,
                        capabilities = EXCLUDED.capabilities,
                        last_seen_at = now()
                    "#,
                )
                .bind(&event.hello.client_id)
                .bind(&event.hello.client_id)
                .bind(public_key)
                .bind(&event.hello.agent_version)
                .bind(&event.hello.os_release)
                .bind(&event.hello.arch)
                .bind(sqlx::types::Json(&event.hello.capabilities))
                .execute(&mut *tx)
                .await?;

                tx.commit().await?;
            }
        }
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
        Ok(())
    }

    pub(crate) async fn record_telemetry(&self, event: &GatewayTelemetryIngest) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let hello = AgentHello {
                    client_id: event.telemetry.client_id.clone(),
                    agent_version: String::new(),
                    os_release: String::new(),
                    arch: String::new(),
                    update_heartbeat: None,
                    capabilities: Default::default(),
                };
                upsert_memory_agent(&memory.agents, &hello).await;
                let mut tunnels = memory.telemetry_tunnels.write().await;
                tunnels.retain(|record| record.client_id != event.telemetry.client_id);
                tunnels.extend(event.telemetry.metrics.tunnels.iter().map(|tunnel| {
                    TelemetryTunnelView {
                        client_id: event.telemetry.client_id.clone(),
                        observed_at: event.telemetry.metrics.observed_unix.to_string(),
                        interface: tunnel.interface.clone(),
                        kind: tunnel.kind.clone(),
                        ownership_mode: tunnel.ownership_mode.clone(),
                        mutation_policy: tunnel.mutation_policy.clone(),
                        promotion_required: tunnel.promotion_required,
                        plan_correlation: if tunnel.plan_id.is_some() || tunnel.plan_name.is_some()
                        {
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
                        mtu: tunnel.mtu.map(|value| value as i64),
                        link_type: tunnel.link_type,
                        address: tunnel.address.clone(),
                        rx_bytes: tunnel.rx_bytes as i64,
                        tx_bytes: tunnel.tx_bytes as i64,
                        traffic_source: tunnel.traffic_source.clone(),
                        traffic_status: tunnel.traffic_status.clone(),
                        traffic_reason: tunnel.traffic_reason.clone(),
                        traffic_checked_unix: tunnel.traffic_checked_unix.map(|value| value as i64),
                        adapter_health: tunnel.adapter_health.as_ref().map(|health| {
                            TelemetryTunnelAdapterHealthView {
                                status: health.status.clone(),
                                checked_unix: health.checked_unix as i64,
                                configured: health.configured,
                                success: health.success,
                                exit_code: health.exit_code,
                                reason: health.reason.clone(),
                                duration_ms: health.duration_ms as i64,
                                command_sha256_hex: health.command_sha256_hex.clone(),
                                timed_out: health.timed_out,
                                output_truncated: health.output_truncated,
                                stdout_sha256_hex: health.stdout_sha256_hex.clone(),
                                stderr_sha256_hex: health.stderr_sha256_hex.clone(),
                            }
                        }),
                    }
                }));
                Ok(())
            }
            Self::Postgres(pool) => {
                let metrics = &event.telemetry.metrics;
                sqlx::query(
                    r#"
                    INSERT INTO telemetry_samples (
                        client_id, observed_at, cpu_load_1, memory_total_bytes,
                        memory_available_bytes, payload
                    )
                    VALUES (
                        $1,
                        to_timestamp($2::double precision),
                        $3,
                        $4,
                        $5,
                        $6
                    )
                    ON CONFLICT (client_id, observed_at) DO UPDATE SET
                        cpu_load_1 = EXCLUDED.cpu_load_1,
                        memory_total_bytes = EXCLUDED.memory_total_bytes,
                        memory_available_bytes = EXCLUDED.memory_available_bytes,
                        payload = EXCLUDED.payload
                    "#,
                )
                .bind(&event.telemetry.client_id)
                .bind(metrics.observed_unix as f64)
                .bind(metrics.cpu.load.one)
                .bind(metrics.memory.total_bytes as i64)
                .bind(metrics.memory.available_bytes as i64)
                .bind(sqlx::types::Json(metrics))
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    UPDATE clients
                    SET status = 'connected', last_seen_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(&event.telemetry.client_id)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }
}

pub(crate) async fn upsert_memory_agent(agents: &Arc<RwLock<Vec<AgentView>>>, hello: &AgentHello) {
    let mut agents = agents.write().await;
    if let Some(agent) = agents.iter_mut().find(|agent| agent.id == hello.client_id) {
        agent.status = "connected".to_string();
        agent.capabilities = hello.capabilities.clone();
        return;
    }
    agents.push(AgentView {
        id: hello.client_id.clone(),
        display_name: hello.client_id.clone(),
        status: "connected".to_string(),
        tags: Vec::new(),
        capabilities: hello.capabilities.clone(),
    });
}
