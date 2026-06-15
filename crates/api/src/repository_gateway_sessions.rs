use anyhow::Result;
use chrono::Utc;
use sqlx::Row;
use vpsman_common::GatewaySessionLifecycleIngest;

use crate::{
    model::GatewaySessionView,
    repository::{MemoryState, Repository},
    unix_now,
};

impl Repository {
    pub(crate) async fn record_gateway_session_started(
        &self,
        event: &GatewaySessionLifecycleIngest,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                if memory
                    .hidden_clients
                    .read()
                    .await
                    .contains(&event.client_id)
                {
                    return Ok(());
                }
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
                    self.record_client_status_webhook_event(
                        &event.client_id,
                        Some(&from_status),
                        "online",
                        "gateway_session_started",
                        gateway_status_metadata(event),
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
                    FROM clients
                    WHERE id = $1 AND hidden_at IS NULL
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
                        id, gateway_id, client_id, noise_public_key_hex, status
                    )
                    VALUES ($1, $2, $3, $4, 'active')
                    ON CONFLICT (id) DO UPDATE SET
                        gateway_id = EXCLUDED.gateway_id,
                        client_id = EXCLUDED.client_id,
                        noise_public_key_hex = EXCLUDED.noise_public_key_hex,
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
                .execute(&mut *tx)
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
                .bind(&event.client_id)
                .bind(event.remote_ip.as_deref())
                .execute(&mut *tx)
                .await?;
                if prior_status != "stale" && prior_status != "online" {
                    crate::repository_ingest::record_client_status_transition_in_tx(
                        &mut tx,
                        &event.client_id,
                        Some(&prior_status),
                        "online",
                        "gateway_session_started",
                        gateway_status_metadata(event),
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
                upsert_memory_gateway_session(memory, event, "ended", event.reason.clone()).await;
                if !memory_has_active_other_session(memory, &event.client_id, event.session_id)
                    .await
                {
                    if let Some(from_status) = set_memory_agent_status(
                        memory,
                        &event.client_id,
                        "disconnected",
                        event.remote_ip.as_deref(),
                        false,
                    )
                    .await
                    {
                        self.record_client_status_webhook_event(
                            &event.client_id,
                            Some(&from_status),
                            "disconnected",
                            "gateway_session_ended",
                            gateway_status_metadata(event),
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
                    FROM clients
                    WHERE id = $1 AND hidden_at IS NULL
                    FOR UPDATE
                    "#,
                )
                .bind(&event.client_id)
                .fetch_optional(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO gateway_sessions (
                        id, gateway_id, client_id, noise_public_key_hex,
                        status, ended_at, end_reason
                    )
                    VALUES ($1, $2, $3, $4, 'ended', now(), $5)
                    ON CONFLICT (id) DO UPDATE SET
                        status = 'ended',
                        last_seen_at = now(),
                        ended_at = COALESCE(gateway_sessions.ended_at, now()),
                        end_reason = EXCLUDED.end_reason
                    "#,
                )
                .bind(event.session_id)
                .bind(&event.gateway_id)
                .bind(&event.client_id)
                .bind(&event.noise_public_key_hex)
                .bind(&event.reason)
                .execute(&mut *tx)
                .await?;
                let update = sqlx::query(
                    r#"
                    UPDATE clients
                    SET
                        status = CASE WHEN status = 'stale' THEN status ELSE 'disconnected' END,
                        registration_ip = COALESCE(registration_ip, $3::inet),
                        last_ip = COALESCE($3::inet, last_ip),
                        last_seen_at = now()
                    WHERE id = $1
                      AND hidden_at IS NULL
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
                .bind(event.remote_ip.as_deref())
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
                                gateway_status_metadata(event),
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
                        gateway_sessions.status,
                        gateway_sessions.started_at::text AS started_at,
                        gateway_sessions.last_seen_at::text AS last_seen_at,
                        gateway_sessions.ended_at::text AS ended_at,
                        gateway_sessions.end_reason
                    FROM gateway_sessions
                    JOIN clients c ON c.id = gateway_sessions.client_id
                    WHERE c.hidden_at IS NULL
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

async fn upsert_memory_gateway_session(
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
        session.status = status.to_string();
        session.last_seen_at = now.clone();
        if status == "ended" {
            session.ended_at = Some(now);
            session.end_reason = end_reason;
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
        started_at: now.clone(),
        last_seen_at: now.clone(),
        ended_at: (status == "ended").then_some(now),
        end_reason,
    });
}

async fn expire_memory_active_other_sessions(
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
    if let Some(agent) = memory
        .agents
        .write()
        .await
        .iter_mut()
        .find(|agent| agent.id == client_id)
    {
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
    changed_from
}

fn gateway_status_metadata(event: &GatewaySessionLifecycleIngest) -> serde_json::Value {
    serde_json::json!({
        "gateway_id": &event.gateway_id,
        "session_id": event.session_id,
        "remote_ip": &event.remote_ip,
        "reason": &event.reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::AgentView, repository::Repository};

    fn session_event(client_id: &str, session_id: uuid::Uuid) -> GatewaySessionLifecycleIngest {
        GatewaySessionLifecycleIngest {
            gateway_id: "gateway-a".to_string(),
            client_id: client_id.to_string(),
            session_id,
            noise_public_key_hex: Some("ab".repeat(32)),
            remote_ip: Some("203.0.113.10".to_string()),
            reason: None,
        }
    }

    #[tokio::test]
    async fn memory_gateway_sessions_do_not_disconnect_newer_active_session() {
        let repo = Repository::Memory(MemoryState::default());
        let Repository::Memory(memory) = &repo else {
            unreachable!();
        };
        memory.agents.write().await.push(AgentView {
            id: "client-a".to_string(),
            display_name: "client-a".to_string(),
            status: "offline".to_string(),
            tags: Vec::new(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            stale_since: None,
            stale_reason: None,
            capabilities: Default::default(),
        });
        let older = uuid::Uuid::new_v4();
        let newer = uuid::Uuid::new_v4();

        repo.record_gateway_session_started(&session_event("client-a", older))
            .await
            .unwrap();
        repo.record_gateway_session_started(&session_event("client-a", newer))
            .await
            .unwrap();
        let sessions = repo.list_gateway_sessions(10).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions
                .iter()
                .find(|session| session.id == older)
                .unwrap()
                .status,
            "expired"
        );
        assert_eq!(
            sessions
                .iter()
                .find(|session| session.id == newer)
                .unwrap()
                .status,
            "active"
        );
        let mut ended = session_event("client-a", older);
        ended.reason = Some("replaced".to_string());
        repo.record_gateway_session_ended(&ended).await.unwrap();

        assert_eq!(memory.agents.read().await[0].status.as_str(), "online");
        assert_eq!(
            memory.agents.read().await[0].registration_ip.as_deref(),
            Some("203.0.113.10")
        );
        assert_eq!(
            memory.agents.read().await[0].last_ip.as_deref(),
            Some("203.0.113.10")
        );
        assert_eq!(repo.list_gateway_sessions(10).await.unwrap().len(), 2);

        repo.record_gateway_session_ended(&session_event("client-a", newer))
            .await
            .unwrap();
        assert_eq!(
            memory.agents.read().await[0].status.as_str(),
            "disconnected"
        );
    }
}
