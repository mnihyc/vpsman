use anyhow::Result;
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
                upsert_memory_gateway_session(memory, event, "active", None).await;
                set_memory_agent_status(memory, &event.client_id, "connected").await;
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
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
                    "UPDATE clients SET status = 'connected', last_seen_at = now() WHERE id = $1",
                )
                .bind(&event.client_id)
                .execute(&mut *tx)
                .await?;
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
                    set_memory_agent_status(memory, &event.client_id, "disconnected").await;
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
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
                sqlx::query(
                    r#"
                    UPDATE clients
                    SET status = 'disconnected', last_seen_at = now()
                    WHERE id = $1
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
                .execute(&mut *tx)
                .await?;
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
                let mut sessions = memory.gateway_sessions.read().await.clone();
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
                        id,
                        gateway_id,
                        client_id,
                        noise_public_key_hex,
                        status,
                        started_at::text AS started_at,
                        last_seen_at::text AS last_seen_at,
                        ended_at::text AS ended_at,
                        end_reason
                    FROM gateway_sessions
                    ORDER BY last_seen_at DESC, id DESC
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

async fn memory_has_active_other_session(
    memory: &MemoryState,
    client_id: &str,
    session_id: uuid::Uuid,
) -> bool {
    memory.gateway_sessions.read().await.iter().any(|session| {
        session.client_id == client_id && session.id != session_id && session.status == "active"
    })
}

async fn set_memory_agent_status(memory: &MemoryState, client_id: &str, status: &str) {
    if let Some(agent) = memory
        .agents
        .write()
        .await
        .iter_mut()
        .find(|agent| agent.id == client_id)
    {
        agent.status = status.to_string();
    }
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
            status: "unknown".to_string(),
            tags: Vec::new(),
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
        let mut ended = session_event("client-a", older);
        ended.reason = Some("replaced".to_string());
        repo.record_gateway_session_ended(&ended).await.unwrap();

        assert_eq!(memory.agents.read().await[0].status.as_str(), "connected");
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
