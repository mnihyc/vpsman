use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::{
    expression_references_vps_rules, insert_tags_into_last_namespace_blocks,
    normalize_tag_namespace_blocks, payload_hash, Expression, VpsRuleContext,
};

use crate::model::*;
use crate::repository::Repository;
use crate::repository_jobs::{
    finish_jobs_in_tx_and_reconcile_event_sources, mark_active_targets_agent_lost_for_client_in_tx,
    skip_suspended_undelivered_targets_for_client_except_in_tx,
    skip_unstarted_queued_targets_for_client_in_tx,
};
use crate::repository_key_lifecycle::{
    lock_postgres_client_lifecycles_in_tx, lock_postgres_definition_lifecycles_in_tx,
    lock_postgres_definitions_and_clients_in_tx, lock_postgres_key_identities_in_tx,
    public_key_sha256_hex, require_visible_postgres_clients_in_tx,
};
use crate::repository_port_forwarding::{
    archive_postgres_port_forwarding_for_agent_delete, lock_postgres_port_forward_client,
    postgres_port_forwarding_blocks_agent_delete,
};
use crate::selector_expression::{
    agent_matches_selector_expression, agent_matches_selector_expression_with_rules,
    parse_selector_expression, vps_rule_contexts_by_client,
};
use crate::unix_now;

const TAG_DISPLAY_ORDER_STEP: i64 = 1024;
const TAG_NATURAL_SORT_SETTING_KEY: &str = "order.namespace_natural_sort_enabled";

impl Repository {
    pub(crate) async fn ensure_visible_display_name_available(
        &self,
        display_name: &str,
        except_client_id: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT id
                    FROM visible_clients
                    WHERE lower(btrim(display_name)) = lower(btrim($1))
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
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        count(*) AS total,
                        count(*) FILTER (WHERE status = 'online' AND last_seen_at IS NOT NULL) AS online,
                        count(*) FILTER (WHERE status IN ('offline', 'disconnected')) AS offline,
                        count(*) FILTER (WHERE status = 'never') AS never,
                        count(*) FILTER (WHERE status = 'suspended') AS suspended,
                        count(*) FILTER (WHERE status = 'revoked') AS revoked,
                        count(*) FILTER (WHERE status = 'stale') AS stale,
                        count(*) FILTER (WHERE (
                            (status = 'online' AND last_seen_at IS NULL)
                            OR status NOT IN ('online', 'offline', 'disconnected', 'never', 'suspended', 'revoked', 'stale')
                        )) AS unknown,
                        count(*) FILTER (
                            WHERE status <> 'suspended'
                              AND NOT (status = 'online' AND last_seen_at IS NOT NULL)
                        ) AS warnings,
                        (SELECT count(*) FROM jobs WHERE status IN ('queued', 'running')) AS running_jobs
                    FROM visible_clients
                    "#,
                )
                .fetch_one(pool)
                .await?;
                Ok(FleetSummary {
                    total: row.try_get::<i64, _>("total")? as usize,
                    online: row.try_get::<i64, _>("online")? as usize,
                    offline: row.try_get::<i64, _>("offline")? as usize,
                    never: row.try_get::<i64, _>("never")? as usize,
                    suspended: row.try_get::<i64, _>("suspended")? as usize,
                    revoked: row.try_get::<i64, _>("revoked")? as usize,
                    unknown: row.try_get::<i64, _>("unknown")? as usize,
                    stale: row.try_get::<i64, _>("stale")? as usize,
                    warnings: row.try_get::<i64, _>("warnings")? as usize,
                    running_jobs: row.try_get::<i64, _>("running_jobs")? as usize,
                })
            }
        }
    }

    pub(crate) async fn list_agents(&self) -> Result<Vec<AgentView>> {
        self.list_agents_by_ids(None).await
    }

    pub(crate) async fn list_agents_for_client_ids(
        &self,
        client_ids: &[String],
    ) -> Result<Vec<AgentView>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.list_agents_by_ids(Some(client_ids)).await
    }

    async fn list_agents_by_ids(&self, client_ids: Option<&[String]>) -> Result<Vec<AgentView>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH tags_by_client AS (
                        SELECT
                            ct.client_id,
                            array_agg(
                                t.name
                                ORDER BY t.display_order, t.created_at, t.name
                            ) AS tags
                        FROM visible_clients tag_client
                        JOIN client_tags ct ON ct.client_id = tag_client.id
                        JOIN tags t ON t.id = ct.tag_id
                        WHERE ($1::text[] IS NULL OR tag_client.id = ANY($1))
                        GROUP BY ct.client_id
                    )
                    SELECT
                        c.id,
                        c.display_name,
                        c.status,
                        host(c.registration_ip) AS registration_ip,
                        host(c.last_ip) AS last_ip,
                        c.last_seen_at::text AS last_seen_at,
                        c.arch,
                        c.internal_build_number,
                        c.process_incarnation_id,
                        c.stale_since::text AS stale_since,
                        c.stale_reason,
                        c.capabilities,
                        COALESCE(
                            tags_by_client.tags,
                            ARRAY[]::TEXT[]
                        ) AS tags
                    FROM visible_clients c
                    LEFT JOIN tags_by_client ON tags_by_client.client_id = c.id
                    WHERE ($1::text[] IS NULL OR c.id = ANY($1))
                    ORDER BY c.display_name, c.id
                    "#,
                )
                .bind(client_ids.map(<[String]>::to_vec))
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
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        t.name,
                        t.display_order,
                        COALESCE(
                            array_remove(array_agg(c.id), NULL),
                            ARRAY[]::TEXT[]
                        ) AS client_ids
                    FROM tags t
                    LEFT JOIN client_tags ct ON ct.tag_id = t.id
                    LEFT JOIN visible_clients c ON c.id = ct.client_id
                    GROUP BY t.id, t.name, t.display_order, t.created_at
                    ORDER BY t.display_order, t.created_at, t.name
                    "#,
                )
                .fetch_all(pool)
                .await?;
                let mut tag_metadata = Vec::<(String, i64)>::new();
                let mut client_ids = Vec::new();
                let mut seen_client_ids = HashSet::new();
                for row in rows {
                    tag_metadata.push((row.try_get("name")?, row.try_get("display_order")?));
                    for client_id in row.try_get::<Vec<String>, _>("client_ids")? {
                        if seen_client_ids.insert(client_id.clone()) {
                            client_ids.push(client_id);
                        }
                    }
                }
                let mut clients_by_tag = HashMap::<String, Vec<AgentView>>::new();
                for agent in self.list_agents_for_client_ids(&client_ids).await? {
                    for name in &agent.tags {
                        clients_by_tag
                            .entry(name.clone())
                            .or_default()
                            .push(agent.clone());
                    }
                }
                Ok(tag_metadata
                    .into_iter()
                    .map(|(name, display_order)| TagView {
                        clients: clients_by_tag.remove(&name).unwrap_or_default(),
                        display_order,
                        name,
                    })
                    .collect())
            }
        }
    }

    pub(crate) async fn tag_order_state(&self) -> Result<TagOrderState> {
        let (ordered_tags, namespace_natural_sort_enabled) = match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                    .execute(&mut *tx)
                    .await?;
                let namespace_natural_sort_enabled =
                    read_postgres_tag_order_setting(&mut tx).await?;
                let rows = sqlx::query(
                    r#"
                    SELECT name, display_order
                    FROM tags
                    ORDER BY display_order, created_at, name
                    "#,
                )
                .fetch_all(&mut *tx)
                .await?;
                let ordered_tags = rows
                    .into_iter()
                    .map(|row| Ok((row.try_get("name")?, row.try_get("display_order")?)))
                    .collect::<Result<Vec<(String, i64)>, sqlx::Error>>()?;
                tx.commit().await?;
                (ordered_tags, namespace_natural_sort_enabled)
            }
        };
        Ok(TagOrderState {
            tags: self.tag_views_for_ordered_tags(&ordered_tags).await?,
            namespace_natural_sort_enabled,
        })
    }

    async fn tag_views_for_ordered_tags(
        &self,
        ordered_tags: &[(String, i64)],
    ) -> Result<Vec<TagView>> {
        let mut current = self
            .list_tags()
            .await?
            .into_iter()
            .map(|tag| (tag.name.clone(), tag))
            .collect::<HashMap<_, _>>();
        Ok(ordered_tags
            .iter()
            .map(|(name, display_order)| {
                let mut tag = current.remove(name).unwrap_or_else(|| TagView {
                    name: name.clone(),
                    display_order: *display_order,
                    clients: Vec::new(),
                });
                tag.display_order = *display_order;
                tag
            })
            .collect())
    }

    pub(crate) async fn create_tag(&self, request: CreateTagRequest) -> Result<TagView> {
        let CreateTagRequest { name, .. } = request;
        self.create_tag_name(name).await
    }

    pub(crate) async fn create_tag_name(&self, name: String) -> Result<TagView> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                ensure_postgres_tags_in_order(&mut tx, std::slice::from_ref(&name)).await?;
                let view = postgres_tag_view_in_tx(&mut tx, &name).await?;
                tx.commit().await?;
                Ok(view)
            }
        }
    }

    pub(crate) async fn update_tag_order(
        &self,
        request: &UpdateTagOrderRequest,
        updated_by: Uuid,
    ) -> Result<TagOrderState> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_tag_order_setting(&mut tx).await?;
                let current = lock_postgres_tags_in_order(&mut tx)
                    .await?
                    .into_iter()
                    .map(|(_, name)| name)
                    .collect::<Vec<_>>();
                let mut ordered = normalize_tag_order(current, &request.ordered_tags)?;
                if request.namespace_natural_sort_enabled {
                    normalize_tag_namespace_blocks(&mut ordered);
                }
                rewrite_postgres_tag_order(&mut tx, &ordered).await?;
                sqlx::query(
                    r#"
                    UPDATE fleet_tag_settings
                    SET value_json = to_jsonb($2::boolean),
                        updated_by = $3,
                        updated_at = clock_timestamp()
                    WHERE setting_key = $1
                    "#,
                )
                .bind(TAG_NATURAL_SORT_SETTING_KEY)
                .bind(request.namespace_natural_sort_enabled)
                .bind(updated_by)
                .execute(&mut *tx)
                .await?;
                let state = Self::postgres_tag_order_state_in_tx(&mut tx).await?;
                tx.commit().await?;
                Ok(state)
            }
        }
    }

    async fn postgres_tag_order_state_in_tx(
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<TagOrderState> {
        let namespace_natural_sort_enabled = read_postgres_tag_order_setting(tx).await?;
        let tag_rows = sqlx::query(
            r#"
            SELECT name, display_order
            FROM tags
            ORDER BY display_order, created_at, name
            "#,
        )
        .fetch_all(&mut **tx)
        .await?;
        let tag_metadata = tag_rows
            .into_iter()
            .map(|row| Ok((row.try_get("name")?, row.try_get("display_order")?)))
            .collect::<Result<Vec<(String, i64)>, sqlx::Error>>()?;
        let agent_rows = sqlx::query(
            r#"
            SELECT
                c.id,
                c.display_name,
                c.status,
                host(c.registration_ip) AS registration_ip,
                host(c.last_ip) AS last_ip,
                c.last_seen_at::text AS last_seen_at,
                c.arch,
                c.internal_build_number,
                c.process_incarnation_id,
                c.stale_since::text AS stale_since,
                c.stale_reason,
                c.capabilities,
                COALESCE(
                    array_remove(
                        array_agg(t.name ORDER BY t.display_order, t.created_at, t.name),
                        NULL
                    ),
                    ARRAY[]::TEXT[]
                ) AS tags
            FROM visible_clients c
            LEFT JOIN client_tags ct ON ct.client_id = c.id
            LEFT JOIN tags t ON t.id = ct.tag_id
            WHERE EXISTS (
                SELECT 1
                FROM client_tags assigned
                WHERE assigned.client_id = c.id
            )
            GROUP BY
                c.id,
                c.display_name,
                c.status,
                c.registration_ip,
                c.last_ip,
                c.last_seen_at,
                c.arch,
                c.internal_build_number,
                c.process_incarnation_id,
                c.stale_since,
                c.stale_reason,
                c.capabilities
            ORDER BY c.display_name, c.id
            "#,
        )
        .fetch_all(&mut **tx)
        .await?;
        let agents = agent_rows
            .into_iter()
            .map(agent_view_from_inventory_row)
            .collect::<Result<Vec<_>>>()?;
        let mut clients_by_tag = HashMap::<String, Vec<AgentView>>::new();
        for agent in agents {
            for tag in &agent.tags {
                clients_by_tag
                    .entry(tag.clone())
                    .or_default()
                    .push(agent.clone());
            }
        }
        Ok(TagOrderState {
            tags: tag_metadata
                .into_iter()
                .map(|(name, display_order)| TagView {
                    clients: clients_by_tag.remove(&name).unwrap_or_default(),
                    display_order,
                    name,
                })
                .collect(),
            namespace_natural_sort_enabled,
        })
    }

    #[cfg(test)]
    pub(crate) async fn assign_agent_tag(&self, client_id: &str, tag: &str) -> Result<TagView> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_definitions_and_clients_in_tx(
                    &mut tx,
                    &[format!("tag:{tag}")],
                    &[client_id.to_string()],
                )
                .await?;
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    &[client_id.to_string()],
                    "agent_not_found",
                )
                .await?;
                ensure_postgres_tags_in_order(&mut tx, &[tag.to_string()]).await?;
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
                let view = postgres_tag_view_in_tx(&mut tx, tag).await?;
                tx.commit().await?;
                Ok(view)
            }
        }
    }

    pub(crate) async fn bulk_mutate_tags(
        &self,
        request: &BulkTagMutationRequest,
        allow_vps_rule_selectors: bool,
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
            .schedule_impacts_for_agent_sets(
                &before_agents,
                &after_agents,
                allow_vps_rule_selectors,
            )
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let target_client_ids = targets
                    .iter()
                    .map(|agent| agent.id.clone())
                    .collect::<Vec<_>>();
                lock_postgres_definitions_and_clients_in_tx(
                    &mut tx,
                    &[format!("tag:{}", request.tag)],
                    &target_client_ids,
                )
                .await?;
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    &target_client_ids,
                    "fixed_targets_not_found",
                )
                .await?;
                if matches!(request.action, BulkTagMutationAction::Add) {
                    ensure_postgres_tags_in_order(&mut tx, std::slice::from_ref(&request.tag))
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
                if changed > 0 {
                    Self::record_postgres_tag_mutation_event_in_tx(
                        &mut tx,
                        tag_action_label(&request.action),
                        &request.tag,
                        &targets,
                    )
                    .await?;
                }
                tx.commit().await?;
                let changed = changed as usize;
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
        allow_vps_rule_selectors: bool,
    ) -> Result<TagMutationResponse> {
        let before_agents = self.list_agents().await?;
        let affected = self.clients_for_tag(tag).await?;
        let target_ids = affected
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let (after_agents, preview_changed) = simulate_remove_tag(&before_agents, &target_ids, tag);
        let schedule_impacts = self
            .schedule_impacts_for_agent_sets(
                &before_agents,
                &after_agents,
                allow_vps_rule_selectors,
            )
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let mut affected_client_ids = affected
                    .iter()
                    .map(|agent| agent.id.clone())
                    .collect::<Vec<_>>();
                affected_client_ids.sort();
                affected_client_ids.dedup();
                lock_postgres_definition_lifecycles_in_tx(&mut tx, &[format!("tag:{tag}")]).await?;
                let current_client_ids = sqlx::query_scalar::<_, String>(
                    r#"
                    SELECT c.id
                    FROM visible_clients c
                    JOIN client_tags ct ON ct.client_id = c.id
                    JOIN tags t ON t.id = ct.tag_id
                    WHERE t.name = $1
                    ORDER BY c.id
                    "#,
                )
                .bind(tag)
                .fetch_all(&mut *tx)
                .await?;
                anyhow::ensure!(
                    current_client_ids == affected_client_ids,
                    "tag_mutation_snapshot_stale"
                );
                lock_postgres_client_lifecycles_in_tx(&mut tx, &affected_client_ids).await?;
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    &affected_client_ids,
                    "tag_mutation_snapshot_stale",
                )
                .await?;
                lock_postgres_tag_order_setting(&mut tx).await?;
                lock_postgres_tags_in_order(&mut tx).await?;
                let result = sqlx::query("DELETE FROM tags WHERE name = $1")
                    .bind(tag)
                    .execute(&mut *tx)
                    .await?;
                let changed = if result.rows_affected() > 0 {
                    affected.len()
                } else {
                    0
                };
                if changed > 0 {
                    Self::record_postgres_tag_mutation_event_in_tx(
                        &mut tx, "delete", tag, &affected,
                    )
                    .await?;
                }
                tx.commit().await?;
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
        allow_vps_rule_selectors: bool,
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
            .schedule_impacts_for_agent_sets(
                &before_agents,
                &after_agents,
                allow_vps_rule_selectors,
            )
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
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_definitions_and_clients_in_tx(
                    &mut tx,
                    &[format!("tag:{tag}")],
                    &[client_id.to_string()],
                )
                .await?;
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    &[client_id.to_string()],
                    "agent_not_found",
                )
                .await?;
                ensure_postgres_tags_in_order(&mut tx, &[tag.to_string()]).await?;
                let changed = sqlx::query(
                    r#"
                    INSERT INTO client_tags (client_id, tag_id)
                    SELECT $1, id FROM tags WHERE name = $2
                    ON CONFLICT DO NOTHING
                    "#,
                )
                .bind(client_id)
                .bind(tag)
                .execute(&mut *tx)
                .await?
                .rows_affected() as usize;
                if changed > 0 {
                    Self::record_postgres_tag_mutation_event_in_tx(
                        &mut tx, "assign", tag, &affected,
                    )
                    .await?;
                }
                tx.commit().await?;
                Ok(tag_mutation_response(
                    tag,
                    "assign",
                    None,
                    affected,
                    changed,
                    schedule_impacts,
                    false,
                ))
            }
        }
    }

    async fn schedule_impacts_for_agent_sets(
        &self,
        before_agents: &[AgentView],
        after_agents: &[AgentView],
        allow_vps_rule_selectors: bool,
    ) -> Result<Vec<ScheduleImpactView>> {
        let schedules = self
            .list_schedules()
            .await?
            .into_iter()
            .filter(|schedule| schedule.enabled && schedule.deleted_at.is_none())
            .map(|schedule| {
                let expression = parse_selector_expression(&schedule.selector_expression)
                    .map_err(|error| anyhow!("invalid schedule selector expression: {error}"))?;
                Ok((schedule, expression))
            })
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(
            allow_vps_rule_selectors
                || !schedules.iter().any(|(_, expression)| {
                    expression
                        .as_ref()
                        .is_some_and(expression_references_vps_rules)
                }),
            "vps_rule_selector_scope_required"
        );
        let rules_by_client = if schedules.iter().any(|(_, expression)| {
            expression
                .as_ref()
                .is_some_and(expression_references_vps_rules)
        }) {
            let mut union = before_agents
                .iter()
                .chain(after_agents)
                .cloned()
                .collect::<Vec<_>>();
            union.sort_by(|left, right| left.id.cmp(&right.id));
            union.dedup_by(|left, right| left.id == right.id);
            self.vps_rule_contexts_for_agents(&union).await?
        } else {
            HashMap::new()
        };
        let mut impacts = Vec::new();
        for (schedule, expression) in schedules {
            let Some(expression) = expression else {
                continue;
            };
            let before_targets = Self::resolve_agents_for_expression_with_rule_contexts(
                before_agents,
                &expression,
                &rules_by_client,
            );
            let after_targets = Self::resolve_agents_for_expression_with_rule_contexts(
                after_agents,
                &expression,
                &rules_by_client,
            );
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

    async fn record_postgres_tag_mutation_event_in_tx(
        tx: &mut Transaction<'_, Postgres>,
        action: &str,
        tag: &str,
        affected: &[AgentView],
    ) -> Result<()> {
        crate::repository_webhook_rules::record_webhook_event_in_tx(
            tx,
            Self::tag_mutation_event(action, tag, affected),
            chrono::Utc::now(),
        )
        .await?;
        Ok(())
    }

    fn tag_mutation_event(
        action: &str,
        tag: &str,
        affected: &[AgentView],
    ) -> crate::model_webhook_rules::WebhookEventCandidate {
        let direction_predicate = match action {
            "add" | "assign" => format!("vps.tag_event.added:{tag}"),
            "remove" | "delete" => format!("vps.tag_event.removed:{tag}"),
            _ => format!("vps.tag_event:{tag}"),
        };
        crate::model_webhook_rules::WebhookEventCandidate {
            kind: "vps.tag_changed".to_string(),
            event_id: format!("vps.tag_changed:{}:{}", Uuid::new_v4(), unix_now()),
            event_predicates: vec![format!("vps.tag_event:{tag}"), direction_predicate],
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
        }
    }

    pub(crate) async fn suspend_agent_with_protected_dispatches(
        &self,
        client_id: &str,
        reason: Option<&str>,
        operator: &AuthContext,
        protected_enqueued_job_ids: &[Uuid],
    ) -> Result<AgentSuspensionMutationResult> {
        let reason = reason
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        anyhow::ensure!(
            reason
                .as_ref()
                .is_none_or(|value| value.chars().count() <= 240),
            "agent_suspend_reason_invalid"
        );
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                // The API installs a 60-second gateway lease before entering
                // this transaction. Server-side budgets ensure a canceled API
                // future cannot leave PostgreSQL waiting past that lease.
                sqlx::query("SELECT set_config('lock_timeout', '10s', true)")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("SELECT set_config('statement_timeout', '25s', true)")
                    .execute(&mut *tx)
                    .await?;
                lock_postgres_client_lifecycles_in_tx(&mut tx, &[client_id.to_string()]).await?;
                crate::repository_policy_lifecycle::lock_client_policy_suppression_in_tx(
                    &mut tx, client_id,
                )
                .await?;
                let row = sqlx::query(
                    r#"
                    SELECT status
                    FROM clients
                    WHERE id=$1 AND hidden_at IS NULL
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?
                .context("agent_not_found")?;
                let prior_status: String = row.try_get("status")?;
                match prior_status.as_str() {
                    "never" | "disconnected" | "offline" | "stale" => {}
                    "suspended" => anyhow::bail!("agent_already_suspended"),
                    "online" => anyhow::bail!("agent_suspend_online"),
                    _ => anyhow::bail!("agent_suspend_ineligible"),
                }
                let row = sqlx::query(
                    r#"
                    UPDATE clients
                    SET status='suspended',
                        suspended_at=clock_timestamp(), suspended_by=$2,
                        suspended_reason=$3, suspended_from_status=$4
                    WHERE id=$1 AND hidden_at IS NULL
                    RETURNING suspended_at::text AS suspended_at, suspended_by,
                              suspended_reason, suspended_from_status
                    "#,
                )
                .bind(client_id)
                .bind(operator.operator.id)
                .bind(&reason)
                .bind(&prior_status)
                .fetch_one(&mut *tx)
                .await?;
                let record = AgentSuspensionRecord {
                    suspended_at: row.try_get("suspended_at")?,
                    suspended_by: row.try_get("suspended_by")?,
                    suspended_reason: row.try_get("suspended_reason")?,
                    suspended_from_status: row.try_get("suspended_from_status")?,
                };
                let skipped_job_ids = skip_suspended_undelivered_targets_for_client_except_in_tx(
                    &mut tx,
                    client_id,
                    "target_suspended",
                    "target_suspended: target skipped because VPS is suspended",
                    protected_enqueued_job_ids,
                )
                .await?;
                finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &skipped_job_ids).await?;
                let resolved_alert_count =
                    crate::repository_policy_lifecycle::suppress_client_policy_alerts_in_tx(
                        &mut tx, client_id,
                    )
                    .await?;
                let transition_metadata = json!({
                    "reason": &reason,
                    "operator_id": operator.operator.id,
                    "result": "suspended",
                    "origin_kind": "operator_request",
                    "component": "inventory-controller",
                });
                sqlx::query(
                    r#"
                    INSERT INTO client_status_history (
                        id, client_id, from_status, to_status, reason, metadata
                    ) VALUES ($1,$2,$3,'suspended','operator_suspended',$4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(client_id)
                .bind(&prior_status)
                .bind(&transition_metadata)
                .execute(&mut *tx)
                .await?;
                crate::repository_ingest::insert_client_status_webhook_event_in_tx(
                    &mut tx,
                    client_id,
                    Some(&prior_status),
                    "suspended",
                    "operator_suspended",
                    transition_metadata.clone(),
                )
                .await?;
                // Suspension and resolved lifecycle rows are the durable
                // authority. Wake the sole delivery owners; they revalidate
                // the exact client/episode immediately before external I/O.
                sqlx::query("SELECT pg_notify('webhook_events', 'alert_notification')")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1,$2,'agent.suspended',$3,NULL,$4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("client:{client_id}"))
                .bind(sqlx::types::Json(json!({
                    "client_id": client_id,
                    "from_status": prior_status,
                    "to_status": "suspended",
                    "reason": &reason,
                    "skipped_unstarted_job_ids": skipped_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                    "resolved_alert_count": resolved_alert_count,
                    "result": "succeeded",
                    "operator_id": operator.operator.id,
                    "operator_username": &operator.operator.username,
                    "operator_role": &operator.operator.role,
                    "operator_session_id": operator.audit_session_id(),
                    "origin_kind": "operator_request",
                    "component": "inventory-controller",
                })))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(AgentSuspensionMutationResult {
                    record: Some(record),
                    skipped_unstarted_job_ids: skipped_job_ids,
                    resolved_alert_count,
                })
            }
        }
    }

    pub(crate) async fn unsuspend_agent(
        &self,
        client_id: &str,
        operator: &AuthContext,
    ) -> Result<AgentSuspensionMutationResult> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_client_lifecycles_in_tx(&mut tx, &[client_id.to_string()]).await?;
                let row = sqlx::query(
                    r#"
                    SELECT status, suspended_from_status
                    FROM clients
                    WHERE id=$1 AND hidden_at IS NULL
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?
                .context("agent_not_found")?;
                let status: String = row.try_get("status")?;
                anyhow::ensure!(status == "suspended", "agent_not_suspended");
                let restored_status: String = row
                    .try_get::<Option<String>, _>("suspended_from_status")?
                    .context("agent_suspension_metadata_invalid")?;
                sqlx::query(
                    r#"
                    UPDATE clients
                    SET status=$2, suspended_at=NULL, suspended_by=NULL,
                        suspended_reason=NULL, suspended_from_status=NULL
                    WHERE id=$1 AND status='suspended' AND hidden_at IS NULL
                    "#,
                )
                .bind(client_id)
                .bind(&restored_status)
                .execute(&mut *tx)
                .await?;
                let transition_metadata = json!({
                    "operator_id": operator.operator.id,
                    "result": &restored_status,
                    "origin_kind": "operator_request",
                    "component": "inventory-controller",
                });
                sqlx::query(
                    r#"
                    INSERT INTO client_status_history (
                        id, client_id, from_status, to_status, reason, metadata
                    ) VALUES ($1,$2,'suspended',$3,'operator_unsuspended',$4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(client_id)
                .bind(&restored_status)
                .bind(&transition_metadata)
                .execute(&mut *tx)
                .await?;
                crate::repository_operational_alerts::reconcile_postgres_agent_alert_transition_in_tx(
                    &mut tx,
                    client_id,
                    &restored_status,
                )
                .await?;
                crate::repository_operational_alerts::mark_postgres_tunnel_alerts_unknown_for_clients_in_tx(
                    &mut tx,
                    &[client_id.to_string()],
                )
                .await?;
                crate::repository_ingest::insert_client_status_webhook_event_in_tx(
                    &mut tx,
                    client_id,
                    Some("suspended"),
                    &restored_status,
                    "operator_unsuspended",
                    transition_metadata,
                )
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1,$2,'agent.unsuspended',$3,NULL,$4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("client:{client_id}"))
                .bind(sqlx::types::Json(json!({
                    "client_id": client_id,
                    "from_status": "suspended",
                    "to_status": &restored_status,
                    "result": "succeeded",
                    "operator_id": operator.operator.id,
                    "operator_username": &operator.operator.username,
                    "operator_role": &operator.operator.role,
                    "operator_session_id": operator.audit_session_id(),
                    "origin_kind": "operator_request",
                    "component": "inventory-controller",
                })))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(AgentSuspensionMutationResult {
                    record: None,
                    skipped_unstarted_job_ids: Vec::new(),
                    resolved_alert_count: 0,
                })
            }
        }
    }

    pub(crate) async fn delete_agent(
        &self,
        client_id: &str,
        reason: Option<&str>,
        operator: &AuthContext,
    ) -> Result<DeleteAgentResult> {
        let reason = reason
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_client_lifecycles_in_tx(&mut tx, &[client_id.to_string()]).await?;
                lock_postgres_port_forward_client(&mut tx, client_id).await?;
                anyhow::ensure!(
                    !postgres_port_forwarding_blocks_agent_delete(&mut tx, client_id).await?,
                    "agent_port_forwarding_cleanup_required"
                );
                let client_row = sqlx::query(
                    r#"
                    SELECT process_incarnation_id, public_key, status
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
                let public_key: Vec<u8> = client_row.try_get("public_key")?;
                let prior_status: String = client_row.try_get("status")?;
                if !public_key.is_empty() {
                    let public_key_sha256_hex = public_key_sha256_hex(&public_key);
                    lock_postgres_key_identities_in_tx(
                        &mut tx,
                        std::slice::from_ref(&public_key_sha256_hex),
                    )
                    .await?;
                    sqlx::query(
                        r#"
                        INSERT INTO client_key_revocations (
                            id, client_id, public_key_sha256_hex, reason, revoked_by
                        )
                        VALUES ($1, $2, $3, 'vps_deleted', $4)
                        ON CONFLICT (public_key_sha256_hex) DO NOTHING
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(client_id)
                    .bind(public_key_sha256_hex)
                    .bind(operator.operator.id)
                    .execute(&mut *tx)
                    .await?;
                }
                let row = sqlx::query(
                    r#"
                    UPDATE clients
                    SET
                        hidden_at = COALESCE(hidden_at, now()),
                        hidden_by = COALESCE(hidden_by, $2),
                        hidden_reason = COALESCE($3, hidden_reason),
                        public_key = ''::bytea,
                        status = 'deleted',
                        process_incarnation_id = NULL,
                        suspended_at = NULL,
                        suspended_by = NULL,
                        suspended_reason = NULL,
                        suspended_from_status = NULL
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
                if prior_status != "deleted" {
                    sqlx::query(
                        r#"
                        INSERT INTO client_status_history (
                            id, client_id, from_status, to_status, reason, metadata
                        )
                        VALUES ($1, $2, $3, 'deleted', 'vps_deleted', $4)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(client_id)
                    .bind(&prior_status)
                    .bind(json!({
                        "reason": &reason,
                        "operator_id": operator.operator.id,
                        "frontend_visible": false,
                    }))
                    .execute(&mut *tx)
                    .await?;
                }
                let archived_port_forward_rule_count =
                    archive_postgres_port_forwarding_for_agent_delete(
                        &mut tx,
                        client_id,
                        operator.operator.id,
                        reason.as_deref(),
                    )
                    .await?;
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
                let soft_deleted_tunnel_plan_rows = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET
                        deleted_at = now(),
                        deleted_by = $2,
                        deleted_reason = $3,
                        enabled = FALSE,
                        builtin_credentials = NULL,
                        updated_at = now()
                    WHERE deleted_at IS NULL
                      AND (left_client_id = $1 OR right_client_id = $1)
                    RETURNING left_client_id, right_client_id
                    "#,
                )
                .bind(client_id)
                .bind(operator.operator.id)
                .bind(&tunnel_delete_reason)
                .fetch_all(&mut *tx)
                .await?;
                let soft_deleted_tunnel_plan_count = soft_deleted_tunnel_plan_rows.len();
                let retired_tunnel_endpoint_pairs = soft_deleted_tunnel_plan_rows
                    .into_iter()
                    .map(|row| {
                        Ok((
                            row.try_get::<String, _>("left_client_id")?,
                            row.try_get::<String, _>("right_client_id")?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut affected_tunnel_clients = retired_tunnel_endpoint_pairs
                    .iter()
                    .flat_map(|(left, right)| [left.clone(), right.clone()])
                    .collect::<Vec<_>>();
                affected_tunnel_clients.sort();
                affected_tunnel_clients.dedup();
                let skipped_job_ids = skip_unstarted_queued_targets_for_client_in_tx(
                    &mut tx,
                    client_id,
                    "vps_deleted",
                    "vps_deleted: target skipped before dispatch",
                )
                .await?;
                let mut affected_job_ids = agent_lost_job_ids.clone();
                affected_job_ids.extend(skipped_job_ids.iter().copied());
                finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &affected_job_ids).await?;
                crate::repository_operational_alerts::reconcile_postgres_agent_alert_transition_in_tx(
                    &mut tx,
                    client_id,
                    "deleted",
                )
                .await?;
                crate::repository_operational_alerts::reconcile_postgres_tunnel_alerts_for_clients_in_tx(
                    &mut tx,
                    &affected_tunnel_clients,
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
                    "related_configuration_and_assignments_preserved": true,
                    "frozen_monitoring_share_targets_preserved": true,
                    "soft_deleted_tunnel_plan_count": soft_deleted_tunnel_plan_count,
                    "archived_port_forward_rule_count": archived_port_forward_rule_count,
                    "agent_lost_job_ids": agent_lost_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                    "skipped_unstarted_job_ids": skipped_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                    "result": "succeeded",
                    "operator_id": operator.operator.id,
                    "operator_username": &operator.operator.username,
                    "operator_role": &operator.operator.role,
                    "operator_session_id": operator.audit_session_id(),
                    "origin_kind": "operator_request",
                    "component": "inventory-controller"
                })))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(DeleteAgentResult {
                    client_id: client_id.to_string(),
                    deleted_at,
                    retired_tunnel_endpoint_pairs,
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    &[client_id.to_string()],
                    "agent_not_found",
                )
                .await?;
                let Some(existing) = sqlx::query(
                    r#"
                    SELECT display_name
                    FROM visible_clients
                    WHERE id = $1
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
                    "result": "succeeded",
                    "operator_id": operator.operator.id,
                    "operator_username": &operator.operator.username,
                    "operator_role": &operator.operator.role,
                    "operator_session_id": operator.audit_session_id(),
                    "origin_kind": "operator_request",
                    "component": "inventory-controller"
                })))
                .execute(&mut *tx)
                .await?;
                let updated = postgres_agent_by_id_in_tx(&mut tx, client_id).await?;
                tx.commit().await?;
                Ok(updated)
            }
        }
    }

    pub(crate) async fn agent_by_id(&self, client_id: &str) -> Result<AgentView> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        c.id,
                        c.display_name,
                        c.status,
                        host(c.registration_ip) AS registration_ip,
                        host(c.last_ip) AS last_ip,
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
                    FROM visible_clients c
                    LEFT JOIN client_tags ct ON ct.client_id = c.id
                    LEFT JOIN tags t ON t.id = ct.tag_id
                    WHERE c.id = $1
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.arch, c.internal_build_number, c.process_incarnation_id, c.stale_since, c.stale_reason, c.capabilities
                    "#,
                )
                .bind(client_id)
                .fetch_one(pool)
                .await?;
                agent_view_from_inventory_row(row)
            }
        }
    }

    pub(crate) async fn resolve_agents_for_expression(
        &self,
        agents: &[AgentView],
        expression: &Expression,
    ) -> Result<Vec<AgentView>> {
        if !expression_references_vps_rules(expression) {
            return Ok(agents
                .iter()
                .filter(|agent| agent_matches_selector_expression(agent, expression))
                .cloned()
                .collect());
        }

        let rules_by_client = self.vps_rule_contexts_for_agents(agents).await?;
        Ok(Self::resolve_agents_for_expression_with_rule_contexts(
            agents,
            expression,
            &rules_by_client,
        ))
    }

    pub(crate) async fn vps_rule_contexts_for_agents(
        &self,
        agents: &[AgentView],
    ) -> Result<HashMap<String, VpsRuleContext>> {
        let client_ids = agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let rows = self.list_all_vps_rules_for_clients(&client_ids).await?;
        Ok(vps_rule_contexts_by_client(&rows))
    }

    pub(crate) fn resolve_agents_for_expression_with_rule_contexts(
        agents: &[AgentView],
        expression: &Expression,
        rules_by_client: &HashMap<String, VpsRuleContext>,
    ) -> Vec<AgentView> {
        agents
            .iter()
            .filter(|agent| {
                agent_matches_selector_expression_with_rules(
                    agent,
                    expression,
                    rules_by_client.get(&agent.id),
                )
            })
            .cloned()
            .collect()
    }

    pub(crate) async fn resolve_agents_for_selector(
        &self,
        agents: &[AgentView],
        selector_expression: &str,
    ) -> Result<Vec<AgentView>> {
        let expression = parse_selector_expression(selector_expression)
            .map_err(|error| anyhow!("invalid selector expression: {error}"))?
            .context("selector expression is empty")?;
        self.resolve_agents_for_expression(agents, &expression)
            .await
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
        let candidates = match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        c.id,
                        c.display_name,
                        c.status,
                        host(c.registration_ip) AS registration_ip,
                        host(c.last_ip) AS last_ip,
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
                    FROM visible_clients c
                    LEFT JOIN client_tags all_ct ON all_ct.client_id = c.id
                    LEFT JOIN tags all_tags ON all_tags.id = all_ct.tag_id
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
            }
        };
        let mut targets = self
            .resolve_agents_for_expression(&candidates, &expression)
            .await?;
        targets.sort_by(|left, right| left.id.cmp(&right.id));
        targets.dedup_by(|left, right| left.id == right.id);
        Ok(BulkResolveResponse {
            target_count: targets.len(),
            targets,
        })
    }
    pub(crate) async fn clients_for_tag(&self, tag: &str) -> Result<Vec<AgentView>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        c.id,
                        c.display_name,
                        c.status,
                        host(c.registration_ip) AS registration_ip,
                        host(c.last_ip) AS last_ip,
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
                    FROM visible_clients c
                    JOIN client_tags matching_ct ON matching_ct.client_id = c.id
                    JOIN tags matching_tag ON matching_tag.id = matching_ct.tag_id
                    LEFT JOIN client_tags all_ct ON all_ct.client_id = c.id
                    LEFT JOIN tags all_tags ON all_tags.id = all_ct.tag_id
                    WHERE matching_tag.name = $1
                    GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.arch, c.internal_build_number, c.process_incarnation_id, c.stale_since, c.stale_reason, c.capabilities
                    ORDER BY c.display_name, c.id
                    "#,
                )
                .bind(tag)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(agent_view_from_inventory_row)
                    .collect()
            }
        }
    }
}

async fn postgres_agent_by_id_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
) -> Result<AgentView> {
    let row = sqlx::query(
        r#"
        SELECT
            c.id,
            c.display_name,
            c.status,
            host(c.registration_ip) AS registration_ip,
            host(c.last_ip) AS last_ip,
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
        FROM visible_clients c
        LEFT JOIN client_tags ct ON ct.client_id = c.id
        LEFT JOIN tags t ON t.id = ct.tag_id
        WHERE c.id = $1
        GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.arch, c.internal_build_number, c.process_incarnation_id, c.stale_since, c.stale_reason, c.capabilities
        "#,
    )
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await?;
    agent_view_from_inventory_row(row)
}

async fn postgres_tag_view_in_tx(tx: &mut Transaction<'_, Postgres>, tag: &str) -> Result<TagView> {
    let display_order: i64 = sqlx::query_scalar("SELECT display_order FROM tags WHERE name = $1")
        .bind(tag)
        .fetch_one(&mut **tx)
        .await?;
    let rows = sqlx::query(
        r#"
        SELECT
            c.id,
            c.display_name,
            c.status,
            host(c.registration_ip) AS registration_ip,
            host(c.last_ip) AS last_ip,
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
        FROM visible_clients c
        JOIN client_tags matching_ct ON matching_ct.client_id = c.id
        JOIN tags matching_tag ON matching_tag.id = matching_ct.tag_id
        LEFT JOIN client_tags all_ct ON all_ct.client_id = c.id
        LEFT JOIN tags all_tags ON all_tags.id = all_ct.tag_id
        WHERE matching_tag.name = $1
        GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.arch, c.internal_build_number, c.process_incarnation_id, c.stale_since, c.stale_reason, c.capabilities
        ORDER BY c.display_name, c.id
        "#,
    )
    .bind(tag)
    .fetch_all(&mut **tx)
    .await?;
    Ok(TagView {
        name: tag.to_string(),
        display_order,
        clients: rows
            .into_iter()
            .map(agent_view_from_inventory_row)
            .collect::<Result<Vec<_>>>()?,
    })
}

fn agent_view_from_inventory_row(row: PgRow) -> Result<AgentView> {
    Ok(AgentView {
        id: row.try_get("id")?,
        display_name: row.try_get("display_name")?,
        status: row.try_get("status")?,
        tags: row.try_get("tags")?,
        registration_ip: row.try_get("registration_ip")?,
        last_ip: row.try_get("last_ip")?,
        last_seen_at: row.try_get("last_seen_at")?,
        arch: row.try_get("arch")?,
        internal_build_number: row.try_get::<i64, _>("internal_build_number")?.max(1) as u64,
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

pub(crate) async fn ensure_postgres_tags_in_order(
    tx: &mut Transaction<'_, Postgres>,
    requested_tags: &[String],
) -> Result<HashMap<String, Uuid>> {
    let namespace_natural_sort_enabled = lock_postgres_tag_order_setting(tx).await?;
    let current_rows = lock_postgres_tags_in_order(tx).await?;
    let current = current_rows
        .iter()
        .map(|(_, name)| name.clone())
        .collect::<Vec<_>>();
    let current_names = current.iter().collect::<HashSet<_>>();
    let mut additions = Vec::new();
    let mut seen_additions = HashSet::new();
    for tag in requested_tags {
        if !current_names.contains(tag) && seen_additions.insert(tag.clone()) {
            additions.push(tag.clone());
        }
    }

    let mut ordered = current.clone();
    insert_tags_into_last_namespace_blocks(
        &mut ordered,
        &additions,
        namespace_natural_sort_enabled,
    );
    for name in &additions {
        let index = ordered
            .iter()
            .position(|candidate| candidate == name)
            .context("new tag missing from computed Postgres order")?;
        sqlx::query(
            r#"
            INSERT INTO tags (id, name, display_order)
            VALUES ($1, $2, $3)
            ON CONFLICT (name) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(name)
        .bind(tag_display_order(index))
        .execute(&mut **tx)
        .await?;
    }
    if ordered != current {
        rewrite_postgres_tag_order(tx, &ordered).await?;
    }

    if requested_tags.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, name
        FROM tags
        WHERE name = ANY($1)
        "#,
    )
    .bind(requested_tags.to_vec())
    .fetch_all(&mut **tx)
    .await?;
    let ids = rows
        .into_iter()
        .map(|row| Ok((row.try_get("name")?, row.try_get("id")?)))
        .collect::<Result<HashMap<String, Uuid>, sqlx::Error>>()?;
    let requested_names = requested_tags.iter().collect::<HashSet<_>>();
    anyhow::ensure!(
        ids.len() == requested_names.len(),
        "tag_order_insert_incomplete"
    );
    Ok(ids)
}

async fn lock_postgres_tag_order_setting(tx: &mut Transaction<'_, Postgres>) -> Result<bool> {
    let value: sqlx::types::Json<Value> = sqlx::query_scalar(
        r#"
        SELECT value_json
        FROM fleet_tag_settings
        WHERE setting_key = $1
        FOR UPDATE
        "#,
    )
    .bind(TAG_NATURAL_SORT_SETTING_KEY)
    .fetch_one(&mut **tx)
    .await?;
    value
        .0
        .as_bool()
        .context("tag order setting must be a JSON boolean")
}

async fn read_postgres_tag_order_setting(tx: &mut Transaction<'_, Postgres>) -> Result<bool> {
    let value: sqlx::types::Json<Value> = sqlx::query_scalar(
        r#"
        SELECT value_json
        FROM fleet_tag_settings
        WHERE setting_key = $1
        "#,
    )
    .bind(TAG_NATURAL_SORT_SETTING_KEY)
    .fetch_one(&mut **tx)
    .await?;
    value
        .0
        .as_bool()
        .context("tag order setting must be a JSON boolean")
}

async fn lock_postgres_tags_in_order(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Vec<(Uuid, String)>> {
    let rows = sqlx::query(
        r#"
        SELECT id, name
        FROM tags
        ORDER BY display_order, created_at, name
        FOR UPDATE
        "#,
    )
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| Ok((row.try_get("id")?, row.try_get("name")?)))
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

async fn rewrite_postgres_tag_order(
    tx: &mut Transaction<'_, Postgres>,
    ordered: &[String],
) -> Result<()> {
    for (index, name) in ordered.iter().enumerate() {
        sqlx::query("UPDATE tags SET display_order = $1 WHERE name = $2")
            .bind(tag_display_order(index))
            .bind(name)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
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
    let omitted = current
        .into_iter()
        .filter(|tag| seen.insert(tag.clone()))
        .collect::<Vec<_>>();
    insert_tags_into_last_namespace_blocks(&mut ordered, &omitted, false);
    Ok(ordered)
}

fn deleted_endpoint_tunnel_plan_reason(client_id: &str, operator_reason: Option<&str>) -> String {
    match operator_reason {
        Some(reason) => format!("endpoint_vps_deleted:{client_id}; operator_reason:{reason}"),
        None => format!("endpoint_vps_deleted:{client_id}"),
    }
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

#[cfg(test)]
mod list_agents_query_tests {
    #[test]
    fn suspension_changes_source_state_and_signals_delivery_consumers() {
        let source = include_str!("repository_inventory.rs");
        let (_, suspend) = source
            .split_once("pub(crate) async fn suspend_agent_with_protected_dispatches")
            .expect("agent suspension producer");
        let (suspend, _) = suspend
            .split_once("pub(crate) async fn unsuspend_agent")
            .expect("agent suspension producer boundary");

        assert!(suspend.contains("suppress_client_policy_alerts_in_tx"));
        assert!(suspend.contains("insert_client_status_webhook_event_in_tx"));
        assert!(suspend.contains("pg_notify('webhook_events', 'alert_notification')"));
        assert!(!suspend.contains("UPDATE fleet_alert_notification_deliveries"));
        assert!(!suspend.contains("UPDATE webhook_rule_deliveries"));
    }

    #[test]
    fn fleet_live_tags_are_ordered_before_they_join_wide_client_rows() {
        let source = include_str!("repository_inventory.rs");
        let (_, query) = source
            .split_once("WITH tags_by_client AS (")
            .expect("agent tag aggregation");
        let (query, _) = query
            .split_once(".bind(client_ids.map")
            .expect("agent query boundary");

        assert!(query.contains("GROUP BY ct.client_id"));
        assert!(query.contains("FROM visible_clients tag_client"));
        assert!(query.contains("JOIN client_tags ct ON ct.client_id = tag_client.id"));
        assert!(query.contains("$1::text[] IS NULL OR tag_client.id = ANY($1)"));
        assert!(query.contains("ORDER BY t.display_order, t.created_at, t.name"));
        assert!(query.contains("COALESCE("));
        assert!(query.contains("tags_by_client.tags"));
        assert!(query.contains("ARRAY[]::TEXT[]"));
        assert!(!query.contains("GROUP BY c.id"));
    }
}
