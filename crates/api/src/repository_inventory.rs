use std::collections::{HashMap, HashSet};

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
                    online: agents
                        .iter()
                        .filter(|agent| agent.status == "online" && !hidden.contains(&agent.id))
                        .count(),
                    offline: agents
                        .iter()
                        .filter(|agent| agent.status == "offline" && !hidden.contains(&agent.id))
                        .count(),
                    stale: agents
                        .iter()
                        .filter(|agent| agent.status == "stale" && !hidden.contains(&agent.id))
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
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL AND status = 'online') AS online,
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL AND status = 'offline') AS offline,
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL AND status = 'stale') AS stale,
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL AND status <> 'online') AS warnings,
                        (SELECT count(*) FROM jobs WHERE status IN ('queued', 'running', 'dispatching')) AS running_jobs
                    "#,
                )
                .fetch_one(pool)
                .await?;
                Ok(FleetSummary {
                    total: row.try_get::<i64, _>("total")? as usize,
                    online: row.try_get::<i64, _>("online")? as usize,
                    offline: row.try_get::<i64, _>("offline")? as usize,
                    stale: row.try_get::<i64, _>("stale")? as usize,
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
                        c.registration_ip::text AS registration_ip,
                        c.last_ip::text AS last_ip,
                        c.last_seen_at::text AS last_seen_at,
                        c.internal_build_number,
                        c.stale_since::text AS stale_since,
                        c.stale_reason,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(t.name ORDER BY t.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    LEFT JOIN client_tags ct ON ct.client_id = c.id
                    LEFT JOIN tags t ON t.id = ct.tag_id
                    WHERE c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.internal_build_number, c.stale_since, c.stale_reason, c.capabilities
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
                            registration_ip: row.try_get("registration_ip")?,
                            last_ip: row.try_get("last_ip")?,
                            last_seen_at: row.try_get("last_seen_at")?,
                            internal_build_number: row
                                .try_get::<i64, _>("internal_build_number")?
                                .max(1) as u64,
                            stale_since: row.try_get("stale_since")?,
                            stale_reason: row.try_get("stale_reason")?,
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
        let CreateTagRequest { name, .. } = request;
        self.create_tag_name(name).await
    }

    pub(crate) async fn create_tag_name(&self, name: String) -> Result<TagView> {
        match self {
            Self::Memory(memory) => {
                let mut tags = memory.tags.write().await;
                if !tags.iter().any(|tag| tag == &name) {
                    tags.push(name.clone());
                    tags.sort();
                }
                Ok(TagView {
                    name,
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
                .bind(&name)
                .execute(pool)
                .await?;
                Ok(TagView {
                    clients: self.clients_for_tag(&name).await?,
                    name,
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
                self.create_tag_name(tag.to_string()).await?;
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

    pub(crate) async fn bulk_mutate_tags(
        &self,
        request: &BulkTagMutationRequest,
    ) -> Result<TagMutationResponse> {
        let before_agents = self.list_agents().await?;
        let targets = self
            .resolve_bulk_targets(&BulkResolveRequest {
                selector_expression: request.selector_expression.clone(),
            })
            .await?
            .targets;
        let target_ids = targets
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let (after_agents, preview_changed) =
            simulate_bulk_tag_mutation(&before_agents, &target_ids, &request.tag, &request.action);
        let schedule_impacts = self
            .schedule_impacts_for_agent_sets(&before_agents, &after_agents)
            .await?;
        if !request.confirmed {
            return Ok(tag_mutation_response(
                &request.tag,
                tag_action_label(&request.action),
                targets,
                preview_changed,
                schedule_impacts,
                true,
            ));
        }
        match self {
            Self::Memory(memory) => {
                let mut changed = 0_usize;
                if matches!(request.action, BulkTagMutationAction::Add) {
                    let mut tags = memory.tags.write().await;
                    if !tags.iter().any(|tag| tag == &request.tag) {
                        tags.push(request.tag.clone());
                        tags.sort();
                    }
                }
                let hidden = memory.hidden_clients.read().await.clone();
                let target_ids = targets
                    .iter()
                    .map(|agent| agent.id.as_str())
                    .collect::<HashSet<_>>();
                let mut agents = memory.agents.write().await;
                for agent in agents.iter_mut().filter(|agent| {
                    !hidden.contains(&agent.id) && target_ids.contains(agent.id.as_str())
                }) {
                    match request.action {
                        BulkTagMutationAction::Add => {
                            if !agent.tags.iter().any(|tag| tag == &request.tag) {
                                agent.tags.push(request.tag.clone());
                                agent.tags.sort();
                                changed += 1;
                            }
                        }
                        BulkTagMutationAction::Remove => {
                            let before = agent.tags.len();
                            agent.tags.retain(|tag| tag != &request.tag);
                            if agent.tags.len() != before {
                                changed += 1;
                            }
                        }
                    }
                }
                if changed > 0 {
                    self.record_tag_mutation_event(
                        tag_action_label(&request.action),
                        &request.tag,
                        &targets,
                    )
                    .await?;
                }
                Ok(tag_mutation_response(
                    &request.tag,
                    tag_action_label(&request.action),
                    targets,
                    changed,
                    schedule_impacts,
                    false,
                ))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                if matches!(request.action, BulkTagMutationAction::Add) {
                    sqlx::query(
                        r#"
                        INSERT INTO tags (id, name)
                        VALUES ($1, $2)
                        ON CONFLICT (name) DO NOTHING
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(&request.tag)
                    .execute(&mut *tx)
                    .await?;
                }
                let mut changed = 0_u64;
                for agent in &targets {
                    match request.action {
                        BulkTagMutationAction::Add => {
                            changed += sqlx::query(
                                r#"
                                INSERT INTO client_tags (client_id, tag_id)
                                SELECT $1, id FROM tags WHERE name = $2
                                ON CONFLICT DO NOTHING
                                "#,
                            )
                            .bind(&agent.id)
                            .bind(&request.tag)
                            .execute(&mut *tx)
                            .await?
                            .rows_affected();
                        }
                        BulkTagMutationAction::Remove => {
                            changed += sqlx::query(
                                r#"
                                DELETE FROM client_tags ct
                                USING tags t
                                WHERE ct.tag_id = t.id
                                  AND ct.client_id = $1
                                  AND t.name = $2
                                "#,
                            )
                            .bind(&agent.id)
                            .bind(&request.tag)
                            .execute(&mut *tx)
                            .await?
                            .rows_affected();
                        }
                    }
                }
                tx.commit().await?;
                let changed = changed as usize;
                if changed > 0 {
                    self.record_tag_mutation_event(
                        tag_action_label(&request.action),
                        &request.tag,
                        &targets,
                    )
                    .await?;
                }
                Ok(tag_mutation_response(
                    &request.tag,
                    tag_action_label(&request.action),
                    targets,
                    changed,
                    schedule_impacts,
                    false,
                ))
            }
        }
    }

    pub(crate) async fn delete_tag(
        &self,
        tag: &str,
        confirmed: bool,
    ) -> Result<TagMutationResponse> {
        let before_agents = self.list_agents().await?;
        let affected = self.clients_for_tag(tag).await?;
        let target_ids = affected
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let (after_agents, preview_changed) = simulate_remove_tag(&before_agents, &target_ids, tag);
        let schedule_impacts = self
            .schedule_impacts_for_agent_sets(&before_agents, &after_agents)
            .await?;
        if !confirmed {
            return Ok(tag_mutation_response(
                tag,
                "delete",
                affected,
                preview_changed,
                schedule_impacts,
                true,
            ));
        }
        match self {
            Self::Memory(memory) => {
                memory.tags.write().await.retain(|existing| existing != tag);
                let mut changed = 0_usize;
                let mut agents = memory.agents.write().await;
                for agent in agents.iter_mut() {
                    let before = agent.tags.len();
                    agent.tags.retain(|existing| existing != tag);
                    if before != agent.tags.len() {
                        changed += 1;
                    }
                }
                if changed > 0 {
                    self.record_tag_mutation_event("delete", tag, &affected)
                        .await?;
                }
                Ok(tag_mutation_response(
                    tag,
                    "delete",
                    affected,
                    changed,
                    schedule_impacts,
                    false,
                ))
            }
            Self::Postgres(pool) => {
                let result = sqlx::query("DELETE FROM tags WHERE name = $1")
                    .bind(tag)
                    .execute(pool)
                    .await?;
                let changed = if result.rows_affected() > 0 {
                    affected.len()
                } else {
                    0
                };
                if changed > 0 {
                    self.record_tag_mutation_event("delete", tag, &affected)
                        .await?;
                }
                Ok(tag_mutation_response(
                    tag,
                    "delete",
                    affected,
                    changed,
                    schedule_impacts,
                    false,
                ))
            }
        }
    }

    pub(crate) async fn assign_agent_tag_mutation(
        &self,
        client_id: &str,
        tag: &str,
        confirmed: bool,
    ) -> Result<TagMutationResponse> {
        let before_agents = self.list_agents().await?;
        let affected = before_agents
            .iter()
            .find(|agent| agent.id == client_id)
            .cloned()
            .with_context(|| format!("agent_not_found:{client_id}"))
            .map(|agent| vec![agent])?;
        let target_ids = HashSet::from([client_id.to_string()]);
        let (after_agents, preview_changed) = simulate_add_tag(&before_agents, &target_ids, tag);
        let schedule_impacts = self
            .schedule_impacts_for_agent_sets(&before_agents, &after_agents)
            .await?;
        if !confirmed {
            return Ok(tag_mutation_response(
                tag,
                "assign",
                affected,
                preview_changed,
                schedule_impacts,
                true,
            ));
        }
        self.assign_agent_tag(client_id, tag).await?;
        if preview_changed > 0 {
            self.record_tag_mutation_event("assign", tag, &affected)
                .await?;
        }
        Ok(tag_mutation_response(
            tag,
            "assign",
            affected,
            preview_changed,
            schedule_impacts,
            false,
        ))
    }

    async fn schedule_impacts_for_agent_sets(
        &self,
        before_agents: &[AgentView],
        after_agents: &[AgentView],
    ) -> Result<Vec<ScheduleImpactView>> {
        let mut impacts = Vec::new();
        for schedule in self
            .list_schedules()
            .await?
            .into_iter()
            .filter(|schedule| schedule.enabled && schedule.deleted_at.is_none())
        {
            let before_targets =
                resolve_agents_from_set(before_agents, &schedule.selector_expression)?;
            let after_targets =
                resolve_agents_from_set(after_agents, &schedule.selector_expression)?;
            let before_ids = before_targets
                .iter()
                .map(|agent| agent.id.clone())
                .collect::<HashSet<_>>();
            let after_ids = after_targets
                .iter()
                .map(|agent| agent.id.clone())
                .collect::<HashSet<_>>();
            if before_ids == after_ids {
                continue;
            }
            let before_by_id = before_targets
                .iter()
                .map(|agent| (agent.id.clone(), agent.clone()))
                .collect::<HashMap<_, _>>();
            let after_by_id = after_targets
                .iter()
                .map(|agent| (agent.id.clone(), agent.clone()))
                .collect::<HashMap<_, _>>();
            let mut added_targets = after_ids
                .difference(&before_ids)
                .filter_map(|id| after_by_id.get(id).cloned())
                .collect::<Vec<_>>();
            let mut removed_targets = before_ids
                .difference(&after_ids)
                .filter_map(|id| before_by_id.get(id).cloned())
                .collect::<Vec<_>>();
            added_targets.sort_by(|left, right| {
                left.display_name
                    .cmp(&right.display_name)
                    .then_with(|| left.id.cmp(&right.id))
            });
            removed_targets.sort_by(|left, right| {
                left.display_name
                    .cmp(&right.display_name)
                    .then_with(|| left.id.cmp(&right.id))
            });
            let unchanged_target_count = before_ids.intersection(&after_ids).count();
            let added_target_count = added_targets.len();
            let removed_target_count = removed_targets.len();
            impacts.push(ScheduleImpactView {
                schedule_id: schedule.id,
                name: schedule.name,
                command_type: schedule.command_type,
                selector_expression: schedule.selector_expression,
                before_target_count: before_ids.len(),
                after_target_count: after_ids.len(),
                added_target_count,
                removed_target_count,
                unchanged_target_count,
                added_targets,
                removed_targets,
                summary: schedule_impact_summary(added_target_count, removed_target_count),
            });
        }
        Ok(impacts)
    }

    async fn record_tag_mutation_event(
        &self,
        action: &str,
        tag: &str,
        affected: &[AgentView],
    ) -> Result<()> {
        let direction_predicate = match action {
            "add" | "assign" => format!("vps.tag_event.added:{tag}"),
            "remove" | "delete" => format!("vps.tag_event.removed:{tag}"),
            _ => format!("vps.tag_event:{tag}"),
        };
        self.record_webhook_event(crate::model_webhook_rules::WebhookEventCandidate {
            kind: "vps.tag_changed".to_string(),
            event_id: format!("vps.tag_changed:{}:{}", Uuid::new_v4(), unix_now()),
            event_predicates: vec![
                format!("vps.tag_event:{tag}"),
                direction_predicate,
            ],
            subject_client_ids: affected.iter().map(|agent| agent.id.clone()).collect(),
            payload: json!({
                "event": {
                    "kind": "vps.tag_changed",
                    "tag": tag,
                    "action": action,
                },
                "vps": affected,
                "tag_mutation": {
                    "action": action,
                    "tag": tag,
                    "affected_client_ids": affected.iter().map(|agent| agent.id.clone()).collect::<Vec<_>>(),
                    "affected_count": affected.len(),
                }
            }),
            actor_id: None,
        })
        .await?;
        Ok(())
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
                        c.registration_ip::text AS registration_ip,
                        c.last_ip::text AS last_ip,
                        c.last_seen_at::text AS last_seen_at,
                        c.internal_build_number,
                        c.stale_since::text AS stale_since,
                        c.stale_reason,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(t.name ORDER BY t.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    LEFT JOIN client_tags ct ON ct.client_id = c.id
                    LEFT JOIN tags t ON t.id = ct.tag_id
                    WHERE c.id = $1 AND c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.internal_build_number, c.stale_since, c.stale_reason, c.capabilities
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
                    registration_ip: row.try_get("registration_ip")?,
                    last_ip: row.try_get("last_ip")?,
                    last_seen_at: row.try_get("last_seen_at")?,
                    internal_build_number: row.try_get::<i64, _>("internal_build_number")?.max(1)
                        as u64,
                    stale_since: row.try_get("stale_since")?,
                    stale_reason: row.try_get("stale_reason")?,
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
                        c.registration_ip::text AS registration_ip,
                        c.last_ip::text AS last_ip,
                        c.last_seen_at::text AS last_seen_at,
                        c.internal_build_number,
                        c.stale_since::text AS stale_since,
                        c.stale_reason,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(all_tags.name ORDER BY all_tags.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    LEFT JOIN client_tags all_ct ON all_ct.client_id = c.id
                    LEFT JOIN tags all_tags ON all_tags.id = all_ct.tag_id
                    WHERE c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.internal_build_number, c.stale_since, c.stale_reason, c.capabilities
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
                            registration_ip: row.try_get("registration_ip")?,
                            last_ip: row.try_get("last_ip")?,
                            last_seen_at: row.try_get("last_seen_at")?,
                            internal_build_number: row
                                .try_get::<i64, _>("internal_build_number")?
                                .max(1) as u64,
                            stale_since: row.try_get("stale_since")?,
                            stale_reason: row.try_get("stale_reason")?,
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
                        c.registration_ip::text AS registration_ip,
                        c.last_ip::text AS last_ip,
                        c.last_seen_at::text AS last_seen_at,
                        c.internal_build_number,
                        c.stale_since::text AS stale_since,
                        c.stale_reason,
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
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.internal_build_number, c.stale_since, c.stale_reason, c.capabilities
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
                            registration_ip: row.try_get("registration_ip")?,
                            last_ip: row.try_get("last_ip")?,
                            last_seen_at: row.try_get("last_seen_at")?,
                            internal_build_number: row
                                .try_get::<i64, _>("internal_build_number")?
                                .max(1) as u64,
                            stale_since: row.try_get("stale_since")?,
                            stale_reason: row.try_get("stale_reason")?,
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

fn tag_action_label(action: &BulkTagMutationAction) -> &'static str {
    match action {
        BulkTagMutationAction::Add => "add",
        BulkTagMutationAction::Remove => "remove",
    }
}

fn tag_mutation_response(
    tag: &str,
    action: &str,
    affected: Vec<AgentView>,
    changed_count: usize,
    schedule_impacts: Vec<ScheduleImpactView>,
    confirmation_required: bool,
) -> TagMutationResponse {
    TagMutationResponse {
        tag: tag.to_string(),
        action: action.to_string(),
        target_count: affected.len(),
        changed_count,
        skipped_count: affected.len().saturating_sub(changed_count),
        affected,
        schedule_impacts,
        confirmation_required,
    }
}

fn simulate_bulk_tag_mutation(
    agents: &[AgentView],
    target_ids: &HashSet<String>,
    tag: &str,
    action: &BulkTagMutationAction,
) -> (Vec<AgentView>, usize) {
    match action {
        BulkTagMutationAction::Add => simulate_add_tag(agents, target_ids, tag),
        BulkTagMutationAction::Remove => simulate_remove_tag(agents, target_ids, tag),
    }
}

fn simulate_add_tag(
    agents: &[AgentView],
    target_ids: &HashSet<String>,
    tag: &str,
) -> (Vec<AgentView>, usize) {
    let mut changed = 0_usize;
    let mut after_agents = agents.to_vec();
    for agent in &mut after_agents {
        if !target_ids.contains(&agent.id) || agent.tags.iter().any(|existing| existing == tag) {
            continue;
        }
        agent.tags.push(tag.to_string());
        agent.tags.sort();
        changed += 1;
    }
    (after_agents, changed)
}

fn simulate_remove_tag(
    agents: &[AgentView],
    target_ids: &HashSet<String>,
    tag: &str,
) -> (Vec<AgentView>, usize) {
    let mut changed = 0_usize;
    let mut after_agents = agents.to_vec();
    for agent in &mut after_agents {
        if !target_ids.contains(&agent.id) {
            continue;
        }
        let before = agent.tags.len();
        agent.tags.retain(|existing| existing != tag);
        if before != agent.tags.len() {
            changed += 1;
        }
    }
    (after_agents, changed)
}

fn resolve_agents_from_set(
    agents: &[AgentView],
    selector_expression: &str,
) -> Result<Vec<AgentView>> {
    let Some(expression) = parse_selector_expression(selector_expression)
        .map_err(|error| anyhow!("invalid schedule selector expression: {error}"))?
    else {
        return Ok(Vec::new());
    };
    let mut targets = agents
        .iter()
        .filter(|agent| agent_matches_selector_expression(agent, &expression))
        .cloned()
        .collect::<Vec<_>>();
    targets.sort_by(|left, right| left.id.cmp(&right.id));
    targets.dedup_by(|left, right| left.id == right.id);
    Ok(targets)
}

fn schedule_impact_summary(added: usize, removed: usize) -> String {
    match (added, removed) {
        (0, 0) => "targets unchanged".to_string(),
        (added, 0) => format!("adds {added} target{}", if added == 1 { "" } else { "s" }),
        (0, removed) => format!(
            "removes {removed} target{}",
            if removed == 1 { "" } else { "s" }
        ),
        (added, removed) => format!(
            "adds {added} target{} and removes {removed} target{}",
            if added == 1 { "" } else { "s" },
            if removed == 1 { "" } else { "s" }
        ),
    }
}
