use anyhow::Result;
use chrono::Utc;
use sqlx::Row;
use vpsman_common::GatewaySessionLifecycleIngest;

use crate::{
    model::{AuditLogView, GatewaySessionView},
    repository::{MemoryState, Repository},
    unix_now,
};

impl Repository {
    pub(crate) async fn active_gateway_session_matches(
        &self,
        gateway_id: &str,
        client_id: &str,
        session_id: uuid::Uuid,
        process_incarnation_id: uuid::Uuid,
    ) -> Result<bool> {
        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(client_id) {
                    return Ok(false);
                }
                let session_matches = memory.gateway_sessions.read().await.iter().any(|session| {
                    session.gateway_id == gateway_id
                        && session.client_id == client_id
                        && session.id == session_id
                        && session.status == "active"
                });
                if !session_matches {
                    return Ok(false);
                }
                Ok(memory.agents.read().await.iter().any(|agent| {
                    agent.id == client_id
                        && !matches!(agent.status.as_str(), "revoked" | "deleted")
                        && agent.process_incarnation_id == Some(process_incarnation_id)
                }))
            }
            Self::Postgres(pool) => {
                let matches: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM gateway_sessions session
                        JOIN visible_clients client ON client.id = session.client_id
                        WHERE session.gateway_id = $1
                          AND session.client_id = $2
                          AND session.id = $3
                          AND session.status = 'active'
                          AND client.status <> 'revoked'
                          AND client.process_incarnation_id = $4
                    )
                    "#,
                )
                .bind(gateway_id)
                .bind(client_id)
                .bind(session_id)
                .bind(process_incarnation_id)
                .fetch_one(pool)
                .await?;
                Ok(matches)
            }
        }
    }

    pub(crate) async fn gateway_session_was_seen(
        &self,
        gateway_id: &str,
        client_id: &str,
        session_id: uuid::Uuid,
    ) -> Result<bool> {
        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(client_id) {
                    return Ok(false);
                }
                Ok(memory.gateway_sessions.read().await.iter().any(|session| {
                    session.gateway_id == gateway_id
                        && session.client_id == client_id
                        && session.id == session_id
                }))
            }
            Self::Postgres(pool) => {
                let matches: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM gateway_sessions session
                        JOIN visible_clients client ON client.id = session.client_id
                        WHERE session.gateway_id = $1
                          AND session.client_id = $2
                          AND session.id = $3
                    )
                    "#,
                )
                .bind(gateway_id)
                .bind(client_id)
                .bind(session_id)
                .fetch_one(pool)
                .await?;
                Ok(matches)
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn record_gateway_session_started(
        &self,
        event: &GatewaySessionLifecycleIngest,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let _key_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                if memory
                    .hidden_clients
                    .read()
                    .await
                    .contains(&event.client_id)
                {
                    return Ok(());
                }
                if memory.agents.read().await.iter().any(|agent| {
                    agent.id == event.client_id
                        && matches!(agent.status.as_str(), "revoked" | "deleted")
                }) {
                    return Ok(());
                }
                if memory
                    .gateway_sessions
                    .read()
                    .await
                    .iter()
                    .any(|session| session.id == event.session_id && session.status != "active")
                {
                    return Ok(());
                }
                let session_boundary_changed =
                    !memory.gateway_sessions.read().await.iter().any(|session| {
                        session.id == event.session_id
                            && session.client_id == event.client_id
                            && session.status == "active"
                    });
                expire_memory_active_other_sessions(memory, &event.client_id, event.session_id)
                    .await;
                upsert_memory_gateway_session(memory, event, "active", None).await;
                if let Some(from_status) = set_memory_agent_status(
                    memory,
                    &event.client_id,
                    "online",
                    event.remote_ip.as_deref(),
                    false,
                )
                .await
                {
                    let metadata = gateway_status_metadata(event, "online");
                    let reason = if from_status == "suspended" {
                        "agent_online_auto_unsuspend"
                    } else {
                        "gateway_session_started"
                    };
                    memory.audits.write().await.push(AuditLogView {
                        id: uuid::Uuid::new_v4(),
                        actor_id: None,
                        action: "agent.status_online".to_string(),
                        target: format!("client:{}", event.client_id),
                        command_hash: None,
                        metadata: metadata.clone(),
                        created_at: unix_now().to_string(),
                    });
                    self.record_client_status_webhook_event(
                        &event.client_id,
                        Some(&from_status),
                        "online",
                        reason,
                        metadata,
                    )
                    .await?;
                } else if session_boundary_changed {
                    self.mark_memory_tunnel_alerts_unknown_for_clients(
                        std::slice::from_ref(&event.client_id),
                        &Utc::now().to_rfc3339(),
                    )
                    .await?;
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                crate::repository_webhook_rules::ensure_webhook_event_partition(pool, Utc::now())
                    .await?;
                let mut tx = pool.begin().await?;
                let prior_status: Option<String> = sqlx::query_scalar(
                    r#"
                    SELECT status
                    FROM visible_clients
                    WHERE id = $1 AND status <> 'revoked'
                    FOR UPDATE
                    "#,
                )
                .bind(&event.client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(prior_status) = prior_status else {
                    tx.commit().await?;
                    return Ok(());
                };
                let existing_session = sqlx::query(
                    "SELECT client_id, status FROM gateway_sessions WHERE id = $1 FOR UPDATE",
                )
                .bind(event.session_id)
                .fetch_optional(&mut *tx)
                .await?;
                if existing_session
                    .as_ref()
                    .is_some_and(|row| row.get::<String, _>("status") != "active")
                {
                    tx.commit().await?;
                    return Ok(());
                }
                let session_boundary_changed = existing_session
                    .as_ref()
                    .is_none_or(|row| row.get::<String, _>("client_id") != event.client_id);
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
                .bind(&event.client_id)
                .bind(event.session_id)
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
                .bind(event.session_id)
                .bind(&event.gateway_id)
                .bind(&event.client_id)
                .bind(&event.noise_public_key_hex)
                .bind(event.remote_ip.as_deref())
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    UPDATE clients
                    SET
                        status = CASE WHEN status = 'stale' THEN status ELSE 'online' END,
                        registration_ip = COALESCE(registration_ip, $2::inet),
                        last_ip = COALESCE($2::inet, last_ip),
                        last_seen_at = now(),
                        suspended_at = NULL,
                        suspended_by = NULL,
                        suspended_reason = NULL,
                        suspended_from_status = NULL
                    WHERE id = $1 AND hidden_at IS NULL
                    "#,
                )
                .bind(&event.client_id)
                .bind(event.remote_ip.as_deref())
                .execute(&mut *tx)
                .await?;
                if prior_status != "stale" && prior_status != "online" {
                    let reason = if prior_status == "suspended" {
                        "agent_online_auto_unsuspend"
                    } else {
                        "gateway_session_started"
                    };
                    crate::repository_ingest::record_client_status_transition_in_tx(
                        &mut tx,
                        &event.client_id,
                        Some(&prior_status),
                        "online",
                        reason,
                        gateway_status_metadata(event, "online"),
                        "gateway_ingest",
                        "gateway-session-lifecycle",
                    )
                    .await?;
                } else if session_boundary_changed {
                    crate::repository_operational_alerts::mark_postgres_tunnel_alerts_unknown_for_clients_in_tx(
                        &mut tx,
                        std::slice::from_ref(&event.client_id),
                    )
                    .await?;
                }
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn record_gateway_session_ended(
        &self,
        event: &GatewaySessionLifecycleIngest,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let _key_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let should_transition_agent = memory
                    .gateway_sessions
                    .read()
                    .await
                    .iter()
                    .find(|session| session.id == event.session_id)
                    .is_none_or(|session| session.status == "active");
                upsert_memory_gateway_session(memory, event, "ended", event.reason.clone()).await;
                if should_transition_agent
                    && !memory_has_active_other_session(memory, &event.client_id, event.session_id)
                        .await
                {
                    if let Some(from_status) = set_memory_agent_status(
                        memory,
                        &event.client_id,
                        "disconnected",
                        None,
                        false,
                    )
                    .await
                    {
                        let metadata = gateway_status_metadata(event, "disconnected");
                        memory.audits.write().await.push(AuditLogView {
                            id: uuid::Uuid::new_v4(),
                            actor_id: None,
                            action: "agent.status_disconnected".to_string(),
                            target: format!("client:{}", event.client_id),
                            command_hash: None,
                            metadata: metadata.clone(),
                            created_at: unix_now().to_string(),
                        });
                        self.record_client_status_webhook_event(
                            &event.client_id,
                            Some(&from_status),
                            "disconnected",
                            "gateway_session_ended",
                            metadata,
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
                let prior_status: Option<String> = sqlx::query_scalar(
                    r#"
                    SELECT status
                    FROM visible_clients
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(&event.client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let prior_session_status: Option<String> = sqlx::query_scalar(
                    "SELECT status FROM gateway_sessions WHERE id = $1 FOR UPDATE",
                )
                .bind(event.session_id)
                .fetch_optional(&mut *tx)
                .await?;
                let should_transition_agent = prior_session_status
                    .as_deref()
                    .is_none_or(|status| status == "active");
                sqlx::query(
                    r#"
                    INSERT INTO gateway_sessions (
                        id, gateway_id, client_id, noise_public_key_hex,
                        remote_ip, status, ended_at, end_reason
                    )
                    VALUES ($1, $2, $3, $4, $5::inet, 'ended', now(), $6)
                    ON CONFLICT (id) DO UPDATE SET
                        remote_ip = COALESCE(EXCLUDED.remote_ip, gateway_sessions.remote_ip),
                        status = 'ended',
                        last_seen_at = now(),
                        ended_at = COALESCE(gateway_sessions.ended_at, now()),
                        end_reason = COALESCE(gateway_sessions.end_reason, EXCLUDED.end_reason)
                    "#,
                )
                .bind(event.session_id)
                .bind(&event.gateway_id)
                .bind(&event.client_id)
                .bind(&event.noise_public_key_hex)
                .bind(event.remote_ip.as_deref())
                .bind(&event.reason)
                .execute(&mut *tx)
                .await?;
                let update = sqlx::query(
                    r#"
                    UPDATE clients
                    SET
                        status = CASE WHEN status = 'stale' THEN status ELSE 'disconnected' END,
                        last_seen_at = now()
                    WHERE id = $1
                      AND hidden_at IS NULL
                      AND status NOT IN ('suspended', 'revoked')
                      AND $3
                      AND NOT EXISTS (
                        SELECT 1
                        FROM gateway_sessions
                        WHERE client_id = $1
                          AND status = 'active'
                          AND id <> $2
                      )
                    "#,
                )
                .bind(&event.client_id)
                .bind(event.session_id)
                .bind(should_transition_agent)
                .execute(&mut *tx)
                .await?;
                if update.rows_affected() > 0 {
                    if let Some(prior_status) = prior_status.as_deref() {
                        if prior_status != "stale" && prior_status != "disconnected" {
                            crate::repository_ingest::record_client_status_transition_in_tx(
                                &mut tx,
                                &event.client_id,
                                Some(prior_status),
                                "disconnected",
                                "gateway_session_ended",
                                gateway_status_metadata(event, "disconnected"),
                                "gateway_ingest",
                                "gateway-session-lifecycle",
                            )
                            .await?;
                        }
                    }
                }
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn list_gateway_sessions(
        &self,
        limit: i64,
    ) -> Result<Vec<GatewaySessionView>> {
        let limit = limit.clamp(1, 200);
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let mut sessions = memory.gateway_sessions.read().await.clone();
                sessions.retain(|session| !hidden.contains(&session.client_id));
                sessions.sort_by(|left, right| {
                    right
                        .last_seen_at
                        .cmp(&left.last_seen_at)
                        .then_with(|| right.id.cmp(&left.id))
                });
                sessions.truncate(limit as usize);
                Ok(sessions)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        gateway_sessions.id,
                        gateway_sessions.gateway_id,
                        gateway_sessions.client_id,
                        gateway_sessions.noise_public_key_hex,
                        host(gateway_sessions.remote_ip) AS remote_ip,
                        c.agent_version,
                        gateway_sessions.status,
                        gateway_sessions.started_at::text AS started_at,
                        gateway_sessions.last_seen_at::text AS last_seen_at,
                        gateway_sessions.ended_at::text AS ended_at,
                        gateway_sessions.end_reason
                    FROM gateway_sessions
                    JOIN visible_clients c ON c.id = gateway_sessions.client_id
                    ORDER BY gateway_sessions.last_seen_at DESC, gateway_sessions.id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(GatewaySessionView {
                            id: row.try_get("id")?,
                            gateway_id: row.try_get("gateway_id")?,
                            client_id: row.try_get("client_id")?,
                            noise_public_key_hex: row.try_get("noise_public_key_hex")?,
                            remote_ip: row.try_get("remote_ip")?,
                            agent_version: row.try_get("agent_version")?,
                            status: row.try_get("status")?,
                            started_at: row.try_get("started_at")?,
                            last_seen_at: row.try_get("last_seen_at")?,
                            ended_at: row.try_get("ended_at")?,
                            end_reason: row.try_get("end_reason")?,
                        })
                    })
                    .collect()
            }
        }
    }
}

pub(crate) async fn upsert_memory_gateway_session(
    memory: &MemoryState,
    event: &GatewaySessionLifecycleIngest,
    status: &str,
    end_reason: Option<String>,
) {
    let now = unix_now().to_string();
    let mut sessions = memory.gateway_sessions.write().await;
    if let Some(session) = sessions
        .iter_mut()
        .find(|session| session.id == event.session_id)
    {
        session.gateway_id = event.gateway_id.clone();
        session.client_id = event.client_id.clone();
        session.noise_public_key_hex = event.noise_public_key_hex.clone();
        session.remote_ip = event.remote_ip.clone();
        if let Some(agent_version) = &event.agent_version {
            session.agent_version = agent_version.clone();
        }
        session.status = status.to_string();
        session.last_seen_at = now.clone();
        if status == "ended" {
            session.ended_at = Some(now);
            if session.end_reason.is_none() {
                session.end_reason = end_reason;
            }
        } else {
            session.ended_at = None;
            session.end_reason = None;
        }
        return;
    }
    sessions.push(GatewaySessionView {
        id: event.session_id,
        gateway_id: event.gateway_id.clone(),
        client_id: event.client_id.clone(),
        status: status.to_string(),
        noise_public_key_hex: event.noise_public_key_hex.clone(),
        remote_ip: event.remote_ip.clone(),
        agent_version: event.agent_version.clone().unwrap_or_default(),
        started_at: now.clone(),
        last_seen_at: now.clone(),
        ended_at: (status == "ended").then_some(now),
        end_reason,
    });
}

pub(crate) async fn expire_memory_active_other_sessions(
    memory: &MemoryState,
    client_id: &str,
    session_id: uuid::Uuid,
) {
    let now = unix_now().to_string();
    let mut sessions = memory.gateway_sessions.write().await;
    for session in sessions.iter_mut() {
        if session.client_id == client_id && session.id != session_id && session.status == "active"
        {
            session.status = "expired".to_string();
            session.last_seen_at = now.clone();
            session.ended_at.get_or_insert_with(|| now.clone());
            session
                .end_reason
                .get_or_insert_with(|| "replaced_by_new_session".to_string());
        }
    }
}

async fn memory_has_active_other_session(
    memory: &MemoryState,
    client_id: &str,
    session_id: uuid::Uuid,
) -> bool {
    memory.gateway_sessions.read().await.iter().any(|session| {
        session.client_id == client_id && session.id != session_id && session.status == "active"
    })
}

async fn set_memory_agent_status(
    memory: &MemoryState,
    client_id: &str,
    status: &str,
    remote_ip: Option<&str>,
    override_stale: bool,
) -> Option<String> {
    if memory.hidden_clients.read().await.contains(client_id) {
        return None;
    }
    let mut changed_from = None;
    {
        let mut agents = memory.agents.write().await;
        let agent = agents.iter_mut().find(|agent| agent.id == client_id)?;
        if matches!(agent.status.as_str(), "revoked" | "deleted") {
            return None;
        }
        if agent.status == "suspended" && status != "online" {
            return None;
        }
        if (override_stale || agent.status != "stale") && agent.status != status {
            changed_from = Some(agent.status.clone());
            agent.status = status.to_string();
        }
        if agent.registration_ip.is_none() {
            agent.registration_ip = remote_ip.map(str::to_string);
        }
        if let Some(remote_ip) = remote_ip {
            agent.last_ip = Some(remote_ip.to_string());
        }
        agent.last_seen_at = Some(unix_now().to_string());
    }
    if changed_from.as_deref() == Some("suspended") && status == "online" {
        memory.agent_suspensions.write().await.remove(client_id);
    }
    changed_from
}

fn gateway_status_metadata(
    event: &GatewaySessionLifecycleIngest,
    result: &str,
) -> serde_json::Value {
    serde_json::json!({
        "gateway_id": &event.gateway_id,
        "gateway_session_id": event.session_id,
        "remote_ip": &event.remote_ip,
        "reason": &event.reason,
        "result": result,
        "origin_kind": "gateway_ingest",
        "component": "gateway-session-lifecycle",
    })
}

#[cfg(test)]
#[path = "tests_repository_gateway_sessions.rs"]
mod tests;
