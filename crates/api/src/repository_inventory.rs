use std::collections::HashSet;

use anyhow::{anyhow, Context, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::model::*;
use crate::repository::Repository;
use crate::selector_expression::{agent_matches_selector_expression, parse_selector_expression};
use crate::unix_now;

impl Repository {
    pub(crate) async fn fleet_summary(&self) -> Result<FleetSummary> {
        match self {
            Self::Memory(memory) => {
                let agents = memory.agents.read().await;
                let hidden = memory.hidden_clients.read().await;
                Ok(FleetSummary {
                    total: agents
                        .iter()
                        .filter(|agent| !hidden.contains(&agent.id))
                        .count(),
                    connected: agents
                        .iter()
                        .filter(|agent| agent.status == "connected" && !hidden.contains(&agent.id))
                        .count(),
                    warnings: 0,
                    running_jobs: 0,
                })
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL) AS total,
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL AND status = 'connected') AS connected,
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL AND status NOT IN ('connected', 'unknown')) AS warnings,
                        (SELECT count(*) FROM jobs WHERE status IN ('queued', 'running', 'dispatching')) AS running_jobs
                    "#,
                )
                .fetch_one(pool)
                .await?;
                Ok(FleetSummary {
                    total: row.try_get::<i64, _>("total")? as usize,
                    connected: row.try_get::<i64, _>("connected")? as usize,
                    warnings: row.try_get::<i64, _>("warnings")? as usize,
                    running_jobs: row.try_get::<i64, _>("running_jobs")? as usize,
                })
            }
        }
    }

    pub(crate) async fn list_agents(&self) -> Result<Vec<AgentView>> {
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                Ok(memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .filter(|agent| !hidden.contains(&agent.id))
                    .cloned()
                    .collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        c.id,
                        c.display_name,
                        c.status,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(t.name ORDER BY t.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    LEFT JOIN client_tags ct ON ct.client_id = c.id
                    LEFT JOIN tags t ON t.id = ct.tag_id
                    WHERE c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.capabilities
                    ORDER BY c.display_name, c.id
                    "#,
                )
                .fetch_all(pool)
                .await?;

                rows.into_iter()
                    .map(|row| {
                        Ok(AgentView {
                            id: row.try_get("id")?,
                            display_name: row.try_get("display_name")?,
                            status: row.try_get("status")?,
                            tags: row.try_get("tags")?,
                            capabilities: row
                                .try_get::<sqlx::types::Json<vpsman_common::AgentCapabilitySnapshot>, _>(
                                    "capabilities",
                                )?
                                .0,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_tags(&self) -> Result<Vec<TagView>> {
        match self {
            Self::Memory(memory) => {
                let mut names: HashSet<String> = memory.tags.read().await.iter().cloned().collect();
                let hidden = memory.hidden_clients.read().await;
                for agent in memory.agents.read().await.iter() {
                    if hidden.contains(&agent.id) {
                        continue;
                    }
                    names.extend(agent.tags.iter().cloned());
                }
                let mut names = names.into_iter().collect::<Vec<_>>();
                names.sort();
                let agents = memory.agents.read().await;
                Ok(names
                    .into_iter()
                    .map(|name| TagView {
                        clients: agents
                            .iter()
                            .filter(|agent| {
                                !hidden.contains(&agent.id)
                                    && agent.tags.iter().any(|tag| tag == &name)
                            })
                            .cloned()
                            .collect(),
                        name,
                    })
                    .collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query("SELECT name FROM tags ORDER BY name")
                    .fetch_all(pool)
                    .await?;
                let mut tags = Vec::with_capacity(rows.len());
                for row in rows {
                    let name: String = row.try_get("name")?;
                    tags.push(TagView {
                        clients: self.clients_for_tag(&name).await?,
                        name,
                    });
                }
                Ok(tags)
            }
        }
    }

    pub(crate) async fn create_tag(&self, request: CreateTagRequest) -> Result<TagView> {
        match self {
            Self::Memory(memory) => {
                let mut tags = memory.tags.write().await;
                if !tags.iter().any(|tag| tag == &request.name) {
                    tags.push(request.name.clone());
                    tags.sort();
                }
                Ok(TagView {
                    name: request.name,
                    clients: Vec::new(),
                })
            }
            Self::Postgres(pool) => {
                let id = Uuid::new_v4();
                sqlx::query(
                    r#"
                    INSERT INTO tags (id, name)
                    VALUES ($1, $2)
                    ON CONFLICT (name) DO NOTHING
                    "#,
                )
                .bind(id)
                .bind(&request.name)
                .execute(pool)
                .await?;
                Ok(TagView {
                    clients: self.clients_for_tag(&request.name).await?,
                    name: request.name,
                })
            }
        }
    }

    pub(crate) async fn assign_agent_tag(&self, client_id: &str, tag: &str) -> Result<TagView> {
        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(client_id) {
                    anyhow::bail!("agent_not_found");
                }
                let mut agents = memory.agents.write().await;
                if let Some(agent) = agents.iter_mut().find(|agent| agent.id == client_id) {
                    if !agent.tags.iter().any(|existing| existing == tag) {
                        agent.tags.push(tag.to_string());
                        agent.tags.sort();
                    }
                }
                drop(agents);
                self.create_tag(CreateTagRequest {
                    name: tag.to_string(),
                })
                .await?;
                let hidden = memory.hidden_clients.read().await;
                Ok(TagView {
                    name: tag.to_string(),
                    clients: memory
                        .agents
                        .read()
                        .await
                        .iter()
                        .filter(|agent| {
                            !hidden.contains(&agent.id)
                                && agent.tags.iter().any(|existing| existing == tag)
                        })
                        .cloned()
                        .collect(),
                })
            }
            Self::Postgres(pool) => {
                let tag_id = Uuid::new_v4();
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO tags (id, name)
                    VALUES ($1, $2)
                    ON CONFLICT (name) DO NOTHING
                    "#,
                )
                .bind(tag_id)
                .bind(tag)
                .execute(&mut *tx)
                .await?;
                let client_exists: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM clients
                        WHERE id = $1 AND hidden_at IS NULL
                    )
                    "#,
                )
                .bind(client_id)
                .fetch_one(&mut *tx)
                .await?;
                anyhow::ensure!(client_exists, "agent_not_found");
                sqlx::query(
                    r#"
                    INSERT INTO client_tags (client_id, tag_id)
                    SELECT $1, id FROM tags WHERE name = $2
                    ON CONFLICT DO NOTHING
                    "#,
                )
                .bind(client_id)
                .bind(tag)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(TagView {
                    name: tag.to_string(),
                    clients: self.clients_for_tag(tag).await?,
                })
            }
        }
    }

    pub(crate) async fn delete_agent(
        &self,
        client_id: &str,
        request: &DeleteAgentRequest,
        operator: &AuthContext,
    ) -> Result<DeleteAgentResponse> {
        let reason = request
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        match self {
            Self::Memory(memory) => {
                let deleted_at = unix_now().to_string();
                let already_hidden = {
                    let mut hidden = memory.hidden_clients.write().await;
                    !hidden.insert(client_id.to_string())
                };
                let mut agents = memory.agents.write().await;
                let found = agents.iter().any(|agent| agent.id == client_id);
                agents.retain(|agent| agent.id != client_id);
                drop(agents);
                anyhow::ensure!(found || already_hidden, "agent_not_found");
                memory.client_public_keys.write().await.remove(client_id);
                for session in memory.gateway_sessions.write().await.iter_mut() {
                    if session.client_id == client_id && session.status == "active" {
                        session.status = "ended".to_string();
                        session.last_seen_at = deleted_at.clone();
                        session.ended_at = Some(deleted_at.clone());
                        session.end_reason = Some("vps_deleted".to_string());
                    }
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "agent.deleted".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata: json!({
                        "reason": reason,
                        "already_hidden": already_hidden,
                        "frontend_visible": false,
                        "access_deactivated": true,
                    }),
                    created_at: deleted_at.clone(),
                });
                Ok(DeleteAgentResponse {
                    client_id: client_id.to_string(),
                    deleted: true,
                    deleted_at,
                })
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE clients
                    SET
                        hidden_at = COALESCE(hidden_at, now()),
                        hidden_by = COALESCE(hidden_by, $2),
                        hidden_reason = COALESCE($3, hidden_reason),
                        status = 'deleted'
                    WHERE id = $1
                    RETURNING id, hidden_at::text AS deleted_at
                    "#,
                )
                .bind(client_id)
                .bind(operator.operator.id)
                .bind(&reason)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    anyhow::bail!("agent_not_found");
                };
                let deleted_at: String = row.try_get("deleted_at")?;
                sqlx::query(
                    r#"
                    UPDATE gateway_sessions
                    SET
                        status = 'ended',
                        last_seen_at = now(),
                        ended_at = COALESCE(ended_at, now()),
                        end_reason = COALESCE(end_reason, 'vps_deleted')
                    WHERE client_id = $1 AND status = 'active'
                    "#,
                )
                .bind(client_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, 'agent.deleted', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("client:{client_id}"))
                .bind(sqlx::types::Json(json!({
                    "reason": reason,
                    "frontend_visible": false,
                    "access_deactivated": true
                })))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(DeleteAgentResponse {
                    client_id: client_id.to_string(),
                    deleted: true,
                    deleted_at,
                })
            }
        }
    }

    pub(crate) async fn update_agent_alias(
        &self,
        client_id: &str,
        display_name: &str,
    ) -> Result<AgentView> {
        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(client_id) {
                    anyhow::bail!("agent_not_found");
                }
                let mut agents = memory.agents.write().await;
                let Some(agent) = agents.iter_mut().find(|agent| agent.id == client_id) else {
                    anyhow::bail!("agent_not_found");
                };
                agent.display_name = display_name.to_string();
                let updated = agent.clone();
                drop(agents);
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let result = sqlx::query(
                    r#"
                    UPDATE clients
                    SET display_name = $2
                    WHERE id = $1 AND hidden_at IS NULL
                    "#,
                )
                .bind(client_id)
                .bind(display_name)
                .execute(pool)
                .await?;
                anyhow::ensure!(result.rows_affected() > 0, "agent_not_found");
                self.agent_by_id(client_id).await
            }
        }
    }

    pub(crate) async fn agent_by_id(&self, client_id: &str) -> Result<AgentView> {
        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(client_id) {
                    anyhow::bail!("agent_not_found:{client_id}");
                }
                memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .find(|agent| agent.id == client_id)
                    .cloned()
                    .with_context(|| format!("agent_not_found:{client_id}"))
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        c.id,
                        c.display_name,
                        c.status,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(t.name ORDER BY t.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    LEFT JOIN client_tags ct ON ct.client_id = c.id
                    LEFT JOIN tags t ON t.id = ct.tag_id
                    WHERE c.id = $1 AND c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.capabilities
                    "#,
                )
                .bind(client_id)
                .fetch_one(pool)
                .await?;
                Ok(AgentView {
                    id: row.try_get("id")?,
                    display_name: row.try_get("display_name")?,
                    status: row.try_get("status")?,
                    tags: row.try_get("tags")?,
                    capabilities: row
                        .try_get::<sqlx::types::Json<vpsman_common::AgentCapabilitySnapshot>, _>(
                            "capabilities",
                        )?
                        .0,
                })
            }
        }
    }

    pub(crate) async fn resolve_bulk_targets(
        &self,
        request: &BulkResolveRequest,
    ) -> Result<BulkResolveResponse> {
        let Some(expression) = parse_selector_expression(&request.selector_expression)
            .map_err(|error| anyhow!("invalid selector expression: {error}"))?
        else {
            return Ok(BulkResolveResponse {
                target_count: 0,
                targets: Vec::new(),
                destructive: request.destructive,
                confirmation_required: request.destructive && !request.confirmed,
            });
        };
        let mut targets = match self {
            Self::Memory(memory) => {
                let agents = memory.agents.read().await;
                let hidden = memory.hidden_clients.read().await;
                agents
                    .iter()
                    .filter(|agent| {
                        !hidden.contains(&agent.id)
                            && agent_matches_selector_expression(agent, &expression)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        c.id,
                        c.display_name,
                        c.status,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(all_tags.name ORDER BY all_tags.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    LEFT JOIN client_tags all_ct ON all_ct.client_id = c.id
                    LEFT JOIN tags all_tags ON all_tags.id = all_ct.tag_id
                    WHERE c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.capabilities
                    ORDER BY c.display_name, c.id
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(AgentView {
                            id: row.try_get("id")?,
                            display_name: row.try_get("display_name")?,
                            status: row.try_get("status")?,
                            tags: row.try_get("tags")?,
                            capabilities: row
                                .try_get::<sqlx::types::Json<vpsman_common::AgentCapabilitySnapshot>, _>(
                                    "capabilities",
                                )?
                                .0,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|agent| agent_matches_selector_expression(agent, &expression))
                    .collect()
            }
        };
        targets.sort_by(|left, right| left.id.cmp(&right.id));
        targets.dedup_by(|left, right| left.id == right.id);
        Ok(BulkResolveResponse {
            target_count: targets.len(),
            targets,
            destructive: request.destructive,
            confirmation_required: request.destructive && !request.confirmed,
        })
    }
    pub(crate) async fn clients_for_tag(&self, tag: &str) -> Result<Vec<AgentView>> {
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                Ok(memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .filter(|agent| {
                        !hidden.contains(&agent.id)
                            && agent.tags.iter().any(|agent_tag| agent_tag == tag)
                    })
                    .cloned()
                    .collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        c.id,
                        c.display_name,
                        c.status,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(all_tags.name ORDER BY all_tags.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    JOIN client_tags matching_ct ON matching_ct.client_id = c.id
                    JOIN tags matching_tag ON matching_tag.id = matching_ct.tag_id
                    LEFT JOIN client_tags all_ct ON all_ct.client_id = c.id
                    LEFT JOIN tags all_tags ON all_tags.id = all_ct.tag_id
                    WHERE matching_tag.name = $1
                      AND c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.capabilities
                    ORDER BY c.display_name, c.id
                    "#,
                )
                .bind(tag)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(AgentView {
                            id: row.try_get("id")?,
                            display_name: row.try_get("display_name")?,
                            status: row.try_get("status")?,
                            tags: row.try_get("tags")?,
                            capabilities: row
                                .try_get::<sqlx::types::Json<vpsman_common::AgentCapabilitySnapshot>, _>(
                                    "capabilities",
                                )?
                                .0,
                        })
                    })
                    .collect()
            }
        }
    }
}
