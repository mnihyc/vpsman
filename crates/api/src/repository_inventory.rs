use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::payload_hash;

use crate::model::*;
use crate::repository::Repository;
use crate::repository_jobs::{
    mark_active_targets_agent_lost_for_client_in_tx, skip_unstarted_queued_targets_for_client_in_tx,
};
use crate::selector_expression::{agent_matches_selector_expression, parse_selector_expression};
use crate::unix_now;

const TAG_DISPLAY_ORDER_STEP: i64 = 1024;

pub(crate) fn display_name_key(display_name: &str) -> String {
    display_name.trim().to_lowercase()
}

impl Repository {
    pub(crate) async fn ensure_visible_display_name_available(
        &self,
        display_name: &str,
        except_client_id: Option<&str>,
    ) -> Result<()> {
        let key = display_name_key(display_name);
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let agents = memory.agents.read().await;
                if agents.iter().any(|agent| {
                    except_client_id.is_none_or(|except| agent.id != except)
                        && !hidden.contains(&agent.id)
                        && display_name_key(&agent.display_name) == key
                }) {
                    anyhow::bail!("display_name_already_exists");
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT id
                    FROM clients
                    WHERE hidden_at IS NULL
                      AND lower(btrim(display_name)) = lower(btrim($1))
                      AND ($2::text IS NULL OR id <> $2)
                    LIMIT 1
                    "#,
                )
                .bind(display_name)
                .bind(except_client_id)
                .fetch_optional(pool)
                .await?;
                if row.is_some() {
                    anyhow::bail!("display_name_already_exists");
                }
                Ok(())
            }
        }
    }

    pub(crate) async fn fixed_target_agents(
        &self,
        target_client_ids: &[String],
    ) -> Result<Vec<AgentView>> {
        let agents = self.list_agents().await?;
        let by_id = agents
            .into_iter()
            .map(|agent| (agent.id.clone(), agent))
            .collect::<HashMap<_, _>>();
        let targets = target_client_ids
            .iter()
            .filter_map(|client_id| by_id.get(client_id).cloned())
            .collect::<Vec<_>>();
        anyhow::ensure!(
            targets.len() == target_client_ids.len(),
            "fixed_targets_not_found"
        );
        Ok(targets)
    }

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
                    never: agents
                        .iter()
                        .filter(|agent| agent.status == "never" && !hidden.contains(&agent.id))
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
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL AND status = 'never') AS never,
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL AND status = 'stale') AS stale,
                        (SELECT count(*) FROM clients WHERE hidden_at IS NULL AND status IN ('offline', 'never', 'stale')) AS warnings,
                        (SELECT count(*) FROM jobs WHERE status IN ('queued', 'running')) AS running_jobs
                    "#,
                )
                .fetch_one(pool)
                .await?;
                Ok(FleetSummary {
                    total: row.try_get::<i64, _>("total")? as usize,
                    online: row.try_get::<i64, _>("online")? as usize,
                    offline: row.try_get::<i64, _>("offline")? as usize,
                    never: row.try_get::<i64, _>("never")? as usize,
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
                let tag_order = memory_tag_order_map(&memory.tags.read().await);
                Ok(memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .filter(|agent| !hidden.contains(&agent.id))
                    .map(|agent| agent_with_ordered_tags(agent, &tag_order))
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
                        c.arch,
                        c.internal_build_number,
                        c.process_incarnation_id,
                        c.stale_since::text AS stale_since,
                        c.stale_reason,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(t.name ORDER BY t.display_order, t.created_at, t.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    LEFT JOIN client_tags ct ON ct.client_id = c.id
                    LEFT JOIN tags t ON t.id = ct.tag_id
                    WHERE c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.arch, c.internal_build_number, c.process_incarnation_id, c.stale_since, c.stale_reason, c.capabilities
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
                            arch: row.try_get("arch")?,
                            internal_build_number: row
                                .try_get::<i64, _>("internal_build_number")?
                                .max(1) as u64,
                            process_incarnation_id: row.try_get("process_incarnation_id")?,
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
                let mut names = memory.tags.read().await.clone();
                let mut seen = names.iter().cloned().collect::<HashSet<_>>();
                let hidden = memory.hidden_clients.read().await;
                let agents = memory.agents.read().await;
                for agent in agents.iter() {
                    if hidden.contains(&agent.id) {
                        continue;
                    }
                    for tag in &agent.tags {
                        if seen.insert(tag.clone()) {
                            names.push(tag.clone());
                        }
                    }
                }
                Ok(names
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| TagView {
                        clients: agents
                            .iter()
                            .filter(|agent| {
                                !hidden.contains(&agent.id)
                                    && agent.tags.iter().any(|tag| tag == &name)
                            })
                            .cloned()
                            .collect(),
                        display_order: tag_display_order(index),
                        name,
                    })
                    .collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    "SELECT name, display_order FROM tags ORDER BY display_order, created_at, name",
                )
                .fetch_all(pool)
                .await?;
                let mut tags = Vec::with_capacity(rows.len());
                for row in rows {
                    let name: String = row.try_get("name")?;
                    tags.push(TagView {
                        clients: self.clients_for_tag(&name).await?,
                        display_order: row.try_get("display_order")?,
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
                let display_order = match tags.iter().position(|tag| tag == &name) {
                    Some(index) => tag_display_order(index),
                    None => {
                        let index = tags.len();
                        tags.push(name.clone());
                        tag_display_order(index)
                    }
                };
                Ok(TagView {
                    name,
                    display_order,
                    clients: Vec::new(),
                })
            }
            Self::Postgres(pool) => {
                let id = Uuid::new_v4();
                sqlx::query(
                    r#"
                    INSERT INTO tags (id, name, display_order)
                    VALUES ($1, $2, (SELECT COALESCE(MAX(display_order), 0) + $3 FROM tags))
                    ON CONFLICT (name) DO NOTHING
                    "#,
                )
                .bind(id)
                .bind(&name)
                .bind(TAG_DISPLAY_ORDER_STEP)
                .execute(pool)
                .await?;
                let row = sqlx::query("SELECT display_order FROM tags WHERE name = $1")
                    .bind(&name)
                    .fetch_one(pool)
                    .await?;
                Ok(TagView {
                    clients: self.clients_for_tag(&name).await?,
                    display_order: row.try_get("display_order")?,
                    name,
                })
            }
        }
    }

    pub(crate) async fn update_tag_order(
        &self,
        request: &UpdateTagOrderRequest,
    ) -> Result<Vec<TagView>> {
        match self {
            Self::Memory(memory) => {
                let current = self.list_tags().await?;
                let ordered = normalize_tag_order(
                    current.iter().map(|tag| tag.name.clone()).collect(),
                    &request.ordered_tags,
                )?;
                *memory.tags.write().await = ordered;
                self.list_tags().await
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let rows = sqlx::query(
                    r#"
                    SELECT name
                    FROM tags
                    ORDER BY display_order, created_at, name
                    FOR UPDATE
                    "#,
                )
                .fetch_all(&mut *tx)
                .await?;
                let current = rows
                    .into_iter()
                    .map(|row| row.try_get("name"))
                    .collect::<Result<Vec<String>, _>>()?;
                let ordered = normalize_tag_order(current, &request.ordered_tags)?;
                for (index, name) in ordered.iter().enumerate() {
                    sqlx::query("UPDATE tags SET display_order = $1 WHERE name = $2")
                        .bind(tag_display_order(index))
                        .bind(name)
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                self.list_tags().await
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
                    }
                }
                drop(agents);
                let tag_view = self.create_tag_name(tag.to_string()).await?;
                let hidden = memory.hidden_clients.read().await;
                Ok(TagView {
                    name: tag.to_string(),
                    display_order: tag_view.display_order,
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
                    INSERT INTO tags (id, name, display_order)
                    VALUES ($1, $2, (SELECT COALESCE(MAX(display_order), 0) + $3 FROM tags))
                    ON CONFLICT (name) DO NOTHING
                    "#,
                )
                .bind(tag_id)
                .bind(tag)
                .bind(TAG_DISPLAY_ORDER_STEP)
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
                let display_order: i64 =
                    sqlx::query_scalar("SELECT display_order FROM tags WHERE name = $1")
                        .bind(tag)
                        .fetch_one(pool)
                        .await?;
                Ok(TagView {
                    name: tag.to_string(),
                    display_order,
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
        let targets = self.fixed_target_agents(&request.target_client_ids).await?;
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
                Some(&request.selector_expression),
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
                    Some(&request.selector_expression),
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
                        INSERT INTO tags (id, name, display_order)
                        VALUES ($1, $2, (SELECT COALESCE(MAX(display_order), 0) + $3 FROM tags))
                        ON CONFLICT (name) DO NOTHING
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(&request.tag)
                    .bind(TAG_DISPLAY_ORDER_STEP)
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
                    Some(&request.selector_expression),
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
                None,
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
                    None,
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
                    None,
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
                None,
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
            None,
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
                let old_process_incarnation_id = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .find(|agent| agent.id == client_id)
                    .and_then(|agent| agent.process_incarnation_id);
                let mut agents = memory.agents.write().await;
                let found = agents.iter().any(|agent| agent.id == client_id);
                agents.retain(|agent| agent.id != client_id);
                drop(agents);
                anyhow::ensure!(found || already_hidden, "agent_not_found");
                memory.client_public_keys.write().await.remove(client_id);
                let tunnel_delete_reason =
                    deleted_endpoint_tunnel_plan_reason(client_id, reason.as_deref());
                let soft_deleted_tunnel_plan_count = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let mut count = 0usize;
                    for plan in plans.iter_mut().filter(|plan| {
                        plan.deleted_at.is_none()
                            && (plan.left_client_id == client_id
                                || plan.right_client_id == client_id)
                    }) {
                        plan.deleted_at = Some(deleted_at.clone());
                        plan.deleted_by = Some(operator.operator.id);
                        plan.deleted_reason = Some(tunnel_delete_reason.clone());
                        plan.enabled = false;
                        plan.updated_at = deleted_at.clone();
                        count += 1;
                    }
                    count
                };
                for session in memory.gateway_sessions.write().await.iter_mut() {
                    if session.client_id == client_id && session.status == "active" {
                        session.status = "ended".to_string();
                        session.last_seen_at = deleted_at.clone();
                        session.ended_at = Some(deleted_at.clone());
                        session.end_reason = Some("vps_deleted".to_string());
                    }
                }
                let agent_lost_job_ids =
                    if let Some(old_process_incarnation_id) = old_process_incarnation_id {
                        self.mark_active_targets_agent_lost_for_client(
                            client_id,
                            old_process_incarnation_id,
                            None,
                            "vps_deleted",
                            "client was deleted before final command output",
                        )
                        .await?
                    } else {
                        Vec::new()
                    };
                let skipped_job_ids = self
                    .skip_unstarted_queued_targets_for_client(
                        client_id,
                        "vps_deleted",
                        "vps_deleted: target skipped before dispatch",
                    )
                    .await?;
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
                        "soft_deleted_tunnel_plan_count": soft_deleted_tunnel_plan_count,
                        "agent_lost_job_ids": agent_lost_job_ids,
                        "skipped_unstarted_job_ids": skipped_job_ids,
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
                let client_row = sqlx::query(
                    r#"
                    SELECT process_incarnation_id
                    FROM clients
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(client_row) = client_row else {
                    anyhow::bail!("agent_not_found");
                };
                let old_process_incarnation_id: Option<Uuid> =
                    client_row.try_get("process_incarnation_id")?;
                let row = sqlx::query(
                    r#"
                    UPDATE clients
                    SET
                        hidden_at = COALESCE(hidden_at, now()),
                        hidden_by = COALESCE(hidden_by, $2),
                        hidden_reason = COALESCE($3, hidden_reason),
                        public_key = ''::bytea,
                        status = 'deleted',
                        process_incarnation_id = NULL
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
                let agent_lost_job_ids =
                    if let Some(old_process_incarnation_id) = old_process_incarnation_id {
                        mark_active_targets_agent_lost_for_client_in_tx(
                            &mut tx,
                            client_id,
                            old_process_incarnation_id,
                            None,
                            "vps_deleted",
                            "client was deleted before final command output",
                        )
                        .await?
                    } else {
                        Vec::new()
                    };
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
                let tunnel_delete_reason =
                    deleted_endpoint_tunnel_plan_reason(client_id, reason.as_deref());
                let soft_deleted_tunnel_plan_count = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET
                        deleted_at = now(),
                        deleted_by = $2,
                        deleted_reason = $3,
                        enabled = FALSE,
                        updated_at = now()
                    WHERE deleted_at IS NULL
                      AND (left_client_id = $1 OR right_client_id = $1)
                    "#,
                )
                .bind(client_id)
                .bind(operator.operator.id)
                .bind(&tunnel_delete_reason)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                let skipped_job_ids = skip_unstarted_queued_targets_for_client_in_tx(
                    &mut tx,
                    client_id,
                    "vps_deleted",
                    "vps_deleted: target skipped before dispatch",
                )
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
                    "access_deactivated": true,
                    "soft_deleted_tunnel_plan_count": soft_deleted_tunnel_plan_count,
                    "agent_lost_job_ids": agent_lost_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                    "skipped_unstarted_job_ids": skipped_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>()
                })))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                let mut job_ids = agent_lost_job_ids;
                job_ids.extend(skipped_job_ids);
                job_ids.sort();
                job_ids.dedup();
                for job_id in job_ids {
                    self.refresh_job_status_from_targets(job_id).await?;
                }
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
        operator: &AuthContext,
    ) -> Result<AgentView> {
        self.ensure_visible_display_name_available(display_name, Some(client_id))
            .await?;
        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(client_id) {
                    anyhow::bail!("agent_not_found");
                }
                let mut agents = memory.agents.write().await;
                let Some(agent) = agents.iter_mut().find(|agent| agent.id == client_id) else {
                    anyhow::bail!("agent_not_found");
                };
                let old_display_name = agent.display_name.clone();
                agent.display_name = display_name.to_string();
                let updated = agent.clone();
                drop(agents);
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "agent.alias_updated".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata: json!({
                        "client_id": client_id,
                        "old_display_name": old_display_name,
                        "new_display_name": display_name,
                    }),
                    created_at: unix_now().to_string(),
                });
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let Some(existing) = sqlx::query(
                    r#"
                    SELECT display_name
                    FROM clients
                    WHERE id = $1 AND hidden_at IS NULL
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?
                else {
                    anyhow::bail!("agent_not_found");
                };
                let old_display_name: String = existing.try_get("display_name")?;
                let row = sqlx::query(
                    r#"
                    UPDATE clients
                    SET display_name = $2
                    WHERE id = $1 AND hidden_at IS NULL
                    RETURNING display_name AS new_display_name
                    "#,
                )
                .bind(client_id)
                .bind(display_name)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    anyhow::bail!("agent_not_found");
                };
                let new_display_name: String = row.try_get("new_display_name")?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, 'agent.alias_updated', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("client:{client_id}"))
                .bind(sqlx::types::Json(json!({
                    "client_id": client_id,
                    "old_display_name": old_display_name,
                    "new_display_name": new_display_name,
                })))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
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
                let tag_order = memory_tag_order_map(&memory.tags.read().await);
                memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .find(|agent| agent.id == client_id)
                    .map(|agent| agent_with_ordered_tags(agent, &tag_order))
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
                        c.arch,
                        c.internal_build_number,
                        c.process_incarnation_id,
                        c.stale_since::text AS stale_since,
                        c.stale_reason,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(t.name ORDER BY t.display_order, t.created_at, t.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    LEFT JOIN client_tags ct ON ct.client_id = c.id
                    LEFT JOIN tags t ON t.id = ct.tag_id
                    WHERE c.id = $1 AND c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.arch, c.internal_build_number, c.process_incarnation_id, c.stale_since, c.stale_reason, c.capabilities
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
                    arch: row.try_get("arch")?,
                    internal_build_number: row.try_get::<i64, _>("internal_build_number")?.max(1)
                        as u64,
                    process_incarnation_id: row.try_get("process_incarnation_id")?,
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
                let tag_order = memory_tag_order_map(&memory.tags.read().await);
                agents
                    .iter()
                    .filter(|agent| {
                        !hidden.contains(&agent.id)
                            && agent_matches_selector_expression(agent, &expression)
                    })
                    .map(|agent| agent_with_ordered_tags(agent, &tag_order))
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
                        c.arch,
                        c.internal_build_number,
                        c.process_incarnation_id,
                        c.stale_since::text AS stale_since,
                        c.stale_reason,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(all_tags.name ORDER BY all_tags.display_order, all_tags.created_at, all_tags.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    LEFT JOIN client_tags all_ct ON all_ct.client_id = c.id
                    LEFT JOIN tags all_tags ON all_tags.id = all_ct.tag_id
                    WHERE c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.arch, c.internal_build_number, c.process_incarnation_id, c.stale_since, c.stale_reason, c.capabilities
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
                            arch: row.try_get("arch")?,
                            internal_build_number: row
                                .try_get::<i64, _>("internal_build_number")?
                                .max(1) as u64,
                            process_incarnation_id: row.try_get("process_incarnation_id")?,
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
                let tag_order = memory_tag_order_map(&memory.tags.read().await);
                Ok(memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .filter(|agent| {
                        !hidden.contains(&agent.id)
                            && agent.tags.iter().any(|agent_tag| agent_tag == tag)
                    })
                    .map(|agent| agent_with_ordered_tags(agent, &tag_order))
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
                        c.arch,
                        c.internal_build_number,
                        c.process_incarnation_id,
                        c.stale_since::text AS stale_since,
                        c.stale_reason,
                        c.capabilities,
                        COALESCE(
                            array_remove(array_agg(all_tags.name ORDER BY all_tags.display_order, all_tags.created_at, all_tags.name), NULL),
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM clients c
                    JOIN client_tags matching_ct ON matching_ct.client_id = c.id
                    JOIN tags matching_tag ON matching_tag.id = matching_ct.tag_id
                    LEFT JOIN client_tags all_ct ON all_ct.client_id = c.id
                    LEFT JOIN tags all_tags ON all_tags.id = all_ct.tag_id
                    WHERE matching_tag.name = $1
                      AND c.hidden_at IS NULL
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.arch, c.internal_build_number, c.process_incarnation_id, c.stale_since, c.stale_reason, c.capabilities
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
                            arch: row.try_get("arch")?,
                            internal_build_number: row
                                .try_get::<i64, _>("internal_build_number")?
                                .max(1) as u64,
                            process_incarnation_id: row.try_get("process_incarnation_id")?,
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
    selector_expression: Option<&str>,
    affected: Vec<AgentView>,
    changed_count: usize,
    schedule_impacts: Vec<ScheduleImpactView>,
    confirmation_required: bool,
) -> TagMutationResponse {
    let preview_hash = tag_mutation_preview_hash(
        tag,
        action,
        selector_expression,
        &affected,
        changed_count,
        &schedule_impacts,
    );
    TagMutationResponse {
        tag: tag.to_string(),
        action: action.to_string(),
        preview_hash,
        target_count: affected.len(),
        changed_count,
        skipped_count: affected.len().saturating_sub(changed_count),
        affected,
        schedule_impacts,
        confirmation_required,
    }
}

fn tag_mutation_preview_hash(
    tag: &str,
    action: &str,
    selector_expression: Option<&str>,
    affected: &[AgentView],
    changed_count: usize,
    schedule_impacts: &[ScheduleImpactView],
) -> String {
    let mut target_client_ids = affected
        .iter()
        .map(|agent| agent.id.as_str())
        .collect::<Vec<_>>();
    target_client_ids.sort_unstable();
    let mut schedule_impacts = schedule_impacts
        .iter()
        .map(|impact| {
            json!({
                "schedule_id": impact.schedule_id,
                "before_target_count": impact.before_target_count,
                "after_target_count": impact.after_target_count,
                "added_target_count": impact.added_target_count,
                "removed_target_count": impact.removed_target_count,
                "unchanged_target_count": impact.unchanged_target_count,
            })
        })
        .collect::<Vec<_>>();
    schedule_impacts.sort_by(|left, right| {
        left.get("schedule_id")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .cmp(
                right
                    .get("schedule_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default(),
            )
    });
    let payload = serde_json::to_vec(&json!({
        "version": 1,
        "action": action,
        "tag": tag,
        "selector_expression": selector_expression,
        "target_client_ids": target_client_ids,
        "target_count": affected.len(),
        "changed_count": changed_count,
        "skipped_count": affected.len().saturating_sub(changed_count),
        "schedule_impacts": schedule_impacts,
    }))
    .unwrap_or_default();
    payload_hash(&payload)
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

fn tag_display_order(index: usize) -> i64 {
    (index as i64 + 1) * TAG_DISPLAY_ORDER_STEP
}

fn normalize_tag_order(current: Vec<String>, requested: &[String]) -> Result<Vec<String>> {
    let current_set = current.iter().cloned().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut ordered = Vec::with_capacity(current.len());
    for tag in requested {
        if !current_set.contains(tag) {
            anyhow::bail!("unknown_tag");
        }
        if !seen.insert(tag.clone()) {
            anyhow::bail!("duplicate_tag");
        }
        ordered.push(tag.clone());
    }
    for tag in current {
        if seen.insert(tag.clone()) {
            ordered.push(tag);
        }
    }
    Ok(ordered)
}

fn memory_tag_order_map(tags: &[String]) -> HashMap<String, usize> {
    tags.iter()
        .enumerate()
        .map(|(index, tag)| (tag.clone(), index))
        .collect()
}

fn agent_with_ordered_tags(agent: &AgentView, tag_order: &HashMap<String, usize>) -> AgentView {
    let mut agent = agent.clone();
    sort_agent_tags_by_order(&mut agent.tags, tag_order);
    agent
}

fn sort_agent_tags_by_order(tags: &mut [String], tag_order: &HashMap<String, usize>) {
    tags.sort_by(|left, right| compare_memory_tags(left, right, tag_order));
}

fn compare_memory_tags(
    left: &str,
    right: &str,
    tag_order: &HashMap<String, usize>,
) -> std::cmp::Ordering {
    tag_order
        .get(left)
        .unwrap_or(&usize::MAX)
        .cmp(tag_order.get(right).unwrap_or(&usize::MAX))
        .then_with(|| left.cmp(right))
}

fn deleted_endpoint_tunnel_plan_reason(client_id: &str, operator_reason: Option<&str>) -> String {
    match operator_reason {
        Some(reason) => format!("endpoint_vps_deleted:{client_id}; operator_reason:{reason}"),
        None => format!("endpoint_vps_deleted:{client_id}"),
    }
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
