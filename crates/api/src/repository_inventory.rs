use std::collections::HashSet;

use anyhow::{Context, Result};
use sqlx::Row;
use uuid::Uuid;

use crate::model::*;
use crate::repository::Repository;

impl Repository {
    pub(crate) async fn fleet_summary(&self) -> Result<FleetSummary> {
        match self {
            Self::Memory(memory) => {
                let agents = memory.agents.read().await;
                Ok(FleetSummary {
                    total: agents.len(),
                    connected: agents
                        .iter()
                        .filter(|agent| agent.status == "connected")
                        .count(),
                    warnings: 0,
                    running_jobs: 0,
                })
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        (SELECT count(*) FROM clients) AS total,
                        (SELECT count(*) FROM clients WHERE status = 'connected') AS connected,
                        (SELECT count(*) FROM clients WHERE status NOT IN ('connected', 'unknown')) AS warnings,
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
            Self::Memory(memory) => Ok(memory.agents.read().await.clone()),
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
                for agent in memory.agents.read().await.iter() {
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
                            .filter(|agent| agent.tags.iter().any(|tag| tag == &name))
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
                Ok(TagView {
                    name: tag.to_string(),
                    clients: memory
                        .agents
                        .read()
                        .await
                        .iter()
                        .filter(|agent| agent.tags.iter().any(|existing| existing == tag))
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

    pub(crate) async fn update_agent_alias(
        &self,
        client_id: &str,
        display_name: &str,
    ) -> Result<AgentView> {
        match self {
            Self::Memory(memory) => {
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
                    WHERE id = $1
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
            Self::Memory(memory) => memory
                .agents
                .read()
                .await
                .iter()
                .find(|agent| agent.id == client_id)
                .cloned()
                .with_context(|| format!("agent_not_found:{client_id}")),
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
                    WHERE c.id = $1
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
        let tag_mode = normalize_bulk_tag_mode(request.tag_mode.as_deref());
        let selectors = bulk_tag_selectors(&request.tags);
        let mut targets = match self {
            Self::Memory(memory) => {
                let agents = memory.agents.read().await;
                agents
                    .iter()
                    .filter(|agent| {
                        request.clients.iter().any(|client| client == &agent.id)
                            || agent_matches_bulk_selectors(agent, &selectors, tag_mode)
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
                    WHERE
                        c.id = ANY($1)
                        OR (
                            $6 = 'any'
                            AND $5 > 0
                            AND (
                                c.id = ANY($2)
                                OR c.display_name = ANY($3)
                                OR EXISTS (
                                    SELECT 1
                                    FROM client_tags matching_ct
                                    JOIN tags matching_tag ON matching_tag.id = matching_ct.tag_id
                                    WHERE matching_ct.client_id = c.id
                                      AND matching_tag.name = ANY($4)
                                )
                            )
                        )
                        OR (
                            $6 = 'all'
                            AND $5 > 0
                            AND (
                                (CASE WHEN c.id = ANY($2) THEN 1 ELSE 0 END)
                                + (CASE WHEN c.display_name = ANY($3) THEN 1 ELSE 0 END)
                                + (
                                    SELECT COUNT(DISTINCT matching_tag.name)::INT
                                    FROM client_tags matching_ct
                                    JOIN tags matching_tag ON matching_tag.id = matching_ct.tag_id
                                    WHERE matching_ct.client_id = c.id
                                      AND matching_tag.name = ANY($4)
                                )
                            ) = $5
                        )
                    GROUP BY c.id, c.display_name, c.status, c.capabilities
                    ORDER BY c.display_name, c.id
                    "#,
                )
                .bind(&request.clients)
                .bind(&selectors.ids)
                .bind(&selectors.names)
                .bind(&selectors.tags)
                .bind(selectors.selector_count)
                .bind(tag_mode)
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
            Self::Memory(memory) => Ok(memory
                .agents
                .read()
                .await
                .iter()
                .filter(|agent| agent.tags.iter().any(|agent_tag| agent_tag == tag))
                .cloned()
                .collect()),
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

fn normalize_bulk_tag_mode(value: Option<&str>) -> &'static str {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("all") | Some("and") => "all",
        _ => "any",
    }
}

#[derive(Debug, Default)]
struct BulkTagSelectors {
    ids: Vec<String>,
    names: Vec<String>,
    tags: Vec<String>,
    selector_count: i32,
}

fn bulk_tag_selectors(tags: &[String]) -> BulkTagSelectors {
    let mut selectors = BulkTagSelectors::default();
    let mut seen = HashSet::new();
    for raw in tags {
        let selector = raw.trim();
        if selector.is_empty() {
            continue;
        }
        if let Some(client_id) = selector.strip_prefix("id:") {
            push_bulk_selector(&mut selectors, &mut seen, "id", client_id);
        } else if let Some(display_name) = selector.strip_prefix("name:") {
            push_bulk_selector(&mut selectors, &mut seen, "name", display_name);
        } else if let Some(tag) = selector.strip_prefix("tag:") {
            push_bulk_selector(&mut selectors, &mut seen, "tag", tag);
        } else {
            push_bulk_selector(&mut selectors, &mut seen, "tag", selector);
        }
    }
    selectors
}

fn push_bulk_selector(
    selectors: &mut BulkTagSelectors,
    seen: &mut HashSet<String>,
    kind: &str,
    value: &str,
) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let key = format!("{kind}\0{value}");
    if !seen.insert(key) {
        return;
    }
    match kind {
        "id" => selectors.ids.push(value.to_string()),
        "name" => selectors.names.push(value.to_string()),
        _ => selectors.tags.push(value.to_string()),
    }
    selectors.selector_count += 1;
}

fn agent_matches_bulk_selectors(
    agent: &AgentView,
    selectors: &BulkTagSelectors,
    tag_mode: &str,
) -> bool {
    if selectors.selector_count == 0 {
        return false;
    }
    if tag_mode == "all" {
        return selectors.ids.iter().all(|id| &agent.id == id)
            && selectors
                .names
                .iter()
                .all(|name| &agent.display_name == name)
            && selectors
                .tags
                .iter()
                .all(|tag| agent.tags.iter().any(|agent_tag| agent_tag == tag));
    }
    selectors.ids.iter().any(|id| &agent.id == id)
        || selectors
            .names
            .iter()
            .any(|name| &agent.display_name == name)
        || selectors
            .tags
            .iter()
            .any(|tag| agent.tags.iter().any(|agent_tag| agent_tag == tag))
}
