use std::collections::HashMap;

use anyhow::Result;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{
    RuntimeTunnelManager, TunnelEndpointSide, TunnelKind, TunnelPlan, TunnelPlanInput,
};

use crate::{
    internal_operator::persisted_actor_id, model::*, repository::Repository,
    repository_key_lifecycle::lock_postgres_agent_identity_lifecycle, unix_now,
};

impl Repository {
    pub(crate) async fn mark_routing_adapter_template_stale(
        &self,
        template_id: Uuid,
    ) -> Result<()> {
        let template_id = template_id.to_string();
        match self {
            Self::Memory(memory) => {
                let mut plans = memory.tunnel_plans.write().await;
                for plan in plans
                    .iter_mut()
                    .filter(|plan| plan.deleted_at.is_none() && plan.enabled)
                {
                    let Some(ospf) = &plan.plan.ospf else {
                        continue;
                    };
                    let left_matches = ospf.left_adapter_template_id == template_id;
                    let right_matches = ospf.right_adapter_template_id == template_id;
                    if !left_matches && !right_matches {
                        continue;
                    }
                    if left_matches {
                        plan.left_ospf_status = "stale".to_string();
                        plan.left_ospf_job_id = None;
                    }
                    if right_matches {
                        plan.right_ospf_status = "stale".to_string();
                        plan.right_ospf_job_id = None;
                    }
                    plan.desired_ospf_cost = None;
                    plan.ospf_status = aggregate_ospf_status(plan).to_string();
                    plan.updated_at = unix_now().to_string();
                }
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET left_ospf_status = CASE
                            WHEN plan->'ospf'->>'left_adapter_template_id' = $1 THEN 'stale'
                            ELSE left_ospf_status
                        END,
                        right_ospf_status = CASE
                            WHEN plan->'ospf'->>'right_adapter_template_id' = $1 THEN 'stale'
                            ELSE right_ospf_status
                        END,
                        left_ospf_job_id = CASE
                            WHEN plan->'ospf'->>'left_adapter_template_id' = $1 THEN NULL
                            ELSE left_ospf_job_id
                        END,
                        right_ospf_job_id = CASE
                            WHEN plan->'ospf'->>'right_adapter_template_id' = $1 THEN NULL
                            ELSE right_ospf_job_id
                        END,
                        desired_ospf_cost = NULL,
                        ospf_status = 'stale',
                        updated_at = now()
                    WHERE deleted_at IS NULL
                      AND enabled = TRUE
                      AND (
                        plan->'ospf'->>'left_adapter_template_id' = $1
                        OR plan->'ospf'->>'right_adapter_template_id' = $1
                      )
                    "#,
                )
                .bind(template_id)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn list_tunnel_plans(&self) -> Result<Vec<TunnelPlanView>> {
        let mut plans = match self {
            Self::Memory(memory) => {
                let mut plans = memory
                    .tunnel_plans
                    .read()
                    .await
                    .iter()
                    .filter(|plan| plan.deleted_at.is_none())
                    .cloned()
                    .collect::<Vec<_>>();
                plans.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                plans
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id, name, kind, enabled, revision, left_client_id, right_client_id,
                        input, plan, recommended_ospf_cost,
                        ospf_status, left_ospf_status, right_ospf_status,
                        desired_ospf_cost, left_current_ospf_cost, right_current_ospf_cost,
                        left_ospf_job_id, right_ospf_job_id,
                        connection_assessment, connection_assessment_note,
                        connection_assessed_at::text AS connection_assessed_at,
                        connection_assessed_by,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at,
                        deleted_at::text AS deleted_at,
                        deleted_by, deleted_reason
                    FROM tunnel_plans
                    WHERE deleted_at IS NULL
                    ORDER BY updated_at DESC, created_at DESC, id DESC
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(tunnel_plan_from_row)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(anyhow::Error::from)?
            }
        };
        let apply_states = self
            .list_runtime_config_apply_records(None)
            .await?
            .into_iter()
            .map(|state| (state.client_id.clone(), state))
            .collect::<HashMap<_, _>>();
        for plan in &mut plans {
            plan.left_runtime_config = tunnel_endpoint_runtime_config_state(
                plan.id,
                &plan.left_client_id,
                plan.enabled,
                apply_states.get(&plan.left_client_id),
            );
            plan.right_runtime_config = tunnel_endpoint_runtime_config_state(
                plan.id,
                &plan.right_client_id,
                plan.enabled,
                apply_states.get(&plan.right_client_id),
            );
        }
        Ok(plans)
    }

    pub(crate) async fn get_tunnel_plan(&self, id: Uuid) -> Result<Option<TunnelPlanView>> {
        Ok(self
            .list_tunnel_plans()
            .await?
            .into_iter()
            .find(|plan| plan.id == id))
    }

    pub(crate) async fn record_tunnel_plan(
        &self,
        input: &TunnelPlanInput,
        plan: &TunnelPlan,
        enabled: bool,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let ospf_endpoint_status = if plan.ospf.is_some() && enabled {
            "unverified"
        } else {
            "disabled"
        };
        let view = TunnelPlanView {
            id: Uuid::new_v4(),
            name: plan.name.clone(),
            kind: plan.kind,
            enabled,
            revision: 1,
            left_client_id: plan.left_client_id.clone(),
            right_client_id: plan.right_client_id.clone(),
            recommended_ospf_cost: plan.recommended_ospf_cost.map(i32::from),
            ospf_status: ospf_endpoint_status.to_string(),
            left_ospf_status: ospf_endpoint_status.to_string(),
            right_ospf_status: ospf_endpoint_status.to_string(),
            desired_ospf_cost: None,
            left_current_ospf_cost: None,
            right_current_ospf_cost: None,
            left_ospf_job_id: None,
            right_ospf_job_id: None,
            connection_assessment: "automatic".to_string(),
            connection_assessment_note: None,
            connection_assessed_at: None,
            connection_assessed_by: None,
            left_runtime_config: untracked_tunnel_runtime_config(&plan.left_client_id, enabled),
            right_runtime_config: untracked_tunnel_runtime_config(&plan.right_client_id, enabled),
            input: input.clone(),
            plan: plan.clone(),
            created_at: unix_now().to_string(),
            updated_at: unix_now().to_string(),
            deleted_at: None,
            deleted_by: None,
            deleted_reason: None,
        };

        let persisted = match self {
            Self::Memory(memory) => {
                let mut plans = memory.tunnel_plans.write().await;
                if plans
                    .iter()
                    .any(|existing| existing.name == view.name && existing.deleted_at.is_none())
                {
                    anyhow::bail!("tunnel_plan_name_conflict");
                }
                plans.push(view.clone());
                view.clone()
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_visible_postgres_tunnel_endpoints(
                    &mut tx,
                    &view.left_client_id,
                    &view.right_client_id,
                )
                .await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO tunnel_plans (
                        id, actor_id, name, kind, enabled,
                        left_client_id, right_client_id, input, plan,
                        recommended_ospf_cost,
                        ospf_status, left_ospf_status, right_ospf_status
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                            $11, $11, $11)
                    ON CONFLICT (name) WHERE deleted_at IS NULL DO NOTHING
                    RETURNING id, revision, created_at::text AS created_at, updated_at::text AS updated_at
                    "#,
                )
                .bind(view.id)
                .bind(persisted_actor_id(operator))
                .bind(&view.name)
                .bind(tunnel_kind_name(view.kind))
                .bind(view.enabled)
                .bind(&view.left_client_id)
                .bind(&view.right_client_id)
                .bind(SqlJson(input))
                .bind(SqlJson(plan))
                .bind(view.recommended_ospf_cost)
                .bind(&view.ospf_status)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| anyhow::anyhow!("tunnel_plan_name_conflict"))?;
                let persisted = TunnelPlanView {
                    id: row.try_get("id")?,
                    revision: row.try_get("revision")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                    ..view.clone()
                };
                insert_tunnel_audit(
                    &mut tx,
                    operator,
                    "network.tunnel_plan_created",
                    &persisted,
                    tunnel_plan_metadata(&persisted, operator),
                )
                .await?;
                tx.commit().await?;
                persisted
            }
        };

        if let Self::Memory(memory) = self {
            memory.audits.write().await.push(AuditLogView {
                id: Uuid::new_v4(),
                actor_id: persisted_actor_id(operator),
                action: "network.tunnel_plan_created".to_string(),
                target: format!("tunnel_plan:{}", persisted.id),
                command_hash: None,
                metadata: tunnel_plan_metadata(&persisted, operator),
                created_at: unix_now().to_string(),
            });
        }
        Ok(persisted)
    }

    pub(crate) async fn update_tunnel_plan(
        &self,
        plan_id: Uuid,
        expected_revision: i64,
        input: &TunnelPlanInput,
        plan: &TunnelPlan,
        enabled: bool,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let ospf_endpoint_status = if plan.ospf.is_some() && enabled {
            "unverified"
        } else {
            "disabled"
        };
        let updated = match self {
            Self::Memory(memory) => {
                let mut plans = memory.tunnel_plans.write().await;
                let existing = plans
                    .iter_mut()
                    .find(|existing| existing.id == plan_id && existing.deleted_at.is_none())
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                if existing.revision != expected_revision {
                    anyhow::bail!("tunnel_plan_snapshot_stale");
                }
                if existing.name != plan.name {
                    anyhow::bail!("tunnel_plan_name_is_immutable");
                }
                let updated = TunnelPlanView {
                    id: existing.id,
                    name: existing.name.clone(),
                    kind: plan.kind,
                    enabled,
                    revision: existing.revision + 1,
                    left_client_id: plan.left_client_id.clone(),
                    right_client_id: plan.right_client_id.clone(),
                    recommended_ospf_cost: plan.recommended_ospf_cost.map(i32::from),
                    ospf_status: ospf_endpoint_status.to_string(),
                    left_ospf_status: ospf_endpoint_status.to_string(),
                    right_ospf_status: ospf_endpoint_status.to_string(),
                    desired_ospf_cost: None,
                    left_current_ospf_cost: None,
                    right_current_ospf_cost: None,
                    left_ospf_job_id: None,
                    right_ospf_job_id: None,
                    connection_assessment: "automatic".to_string(),
                    connection_assessment_note: None,
                    connection_assessed_at: None,
                    connection_assessed_by: None,
                    left_runtime_config: untracked_tunnel_runtime_config(
                        &plan.left_client_id,
                        enabled,
                    ),
                    right_runtime_config: untracked_tunnel_runtime_config(
                        &plan.right_client_id,
                        enabled,
                    ),
                    input: input.clone(),
                    plan: plan.clone(),
                    created_at: existing.created_at.clone(),
                    updated_at: unix_now().to_string(),
                    deleted_at: None,
                    deleted_by: None,
                    deleted_reason: None,
                };
                *existing = updated.clone();
                updated
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_visible_postgres_tunnel_endpoints(
                    &mut tx,
                    &plan.left_client_id,
                    &plan.right_client_id,
                )
                .await?;
                let row = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET actor_id = $1,
                        kind = $2,
                        enabled = $3,
                        left_client_id = $4,
                        right_client_id = $5,
                        input = $6,
                        plan = $7,
                        recommended_ospf_cost = $8,
                        ospf_status = $9,
                        left_ospf_status = $9,
                        right_ospf_status = $9,
                        desired_ospf_cost = NULL,
                        left_current_ospf_cost = NULL,
                        right_current_ospf_cost = NULL,
                        left_ospf_job_id = NULL,
                        right_ospf_job_id = NULL,
                        connection_assessment = 'automatic',
                        connection_assessment_note = NULL,
                        connection_assessed_at = NULL,
                        connection_assessed_by = NULL,
                        revision = revision + 1,
                        updated_at = now()
                    WHERE id = $10
                      AND deleted_at IS NULL
                      AND name = $11
                      AND revision = $12
                    RETURNING revision, created_at::text AS created_at, updated_at::text AS updated_at
                    "#,
                )
                .bind(persisted_actor_id(operator))
                .bind(tunnel_kind_name(plan.kind))
                .bind(enabled)
                .bind(&plan.left_client_id)
                .bind(&plan.right_client_id)
                .bind(SqlJson(input))
                .bind(SqlJson(plan))
                .bind(plan.recommended_ospf_cost.map(i32::from))
                .bind(ospf_endpoint_status)
                .bind(plan_id)
                .bind(&plan.name)
                .bind(expected_revision)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| anyhow::anyhow!("tunnel_plan_snapshot_stale"))?;
                let updated = TunnelPlanView {
                    id: plan_id,
                    name: plan.name.clone(),
                    kind: plan.kind,
                    enabled,
                    revision: row.try_get("revision")?,
                    left_client_id: plan.left_client_id.clone(),
                    right_client_id: plan.right_client_id.clone(),
                    recommended_ospf_cost: plan.recommended_ospf_cost.map(i32::from),
                    ospf_status: ospf_endpoint_status.to_string(),
                    left_ospf_status: ospf_endpoint_status.to_string(),
                    right_ospf_status: ospf_endpoint_status.to_string(),
                    desired_ospf_cost: None,
                    left_current_ospf_cost: None,
                    right_current_ospf_cost: None,
                    left_ospf_job_id: None,
                    right_ospf_job_id: None,
                    connection_assessment: "automatic".to_string(),
                    connection_assessment_note: None,
                    connection_assessed_at: None,
                    connection_assessed_by: None,
                    left_runtime_config: untracked_tunnel_runtime_config(
                        &plan.left_client_id,
                        enabled,
                    ),
                    right_runtime_config: untracked_tunnel_runtime_config(
                        &plan.right_client_id,
                        enabled,
                    ),
                    input: input.clone(),
                    plan: plan.clone(),
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                    deleted_at: None,
                    deleted_by: None,
                    deleted_reason: None,
                };
                insert_tunnel_audit(
                    &mut tx,
                    operator,
                    "network.tunnel_plan_updated",
                    &updated,
                    tunnel_plan_metadata(&updated, operator),
                )
                .await?;
                tx.commit().await?;
                updated
            }
        };

        if let Self::Memory(memory) = self {
            memory.audits.write().await.push(AuditLogView {
                id: Uuid::new_v4(),
                actor_id: persisted_actor_id(operator),
                action: "network.tunnel_plan_updated".to_string(),
                target: format!("tunnel_plan:{}", updated.id),
                command_hash: None,
                metadata: tunnel_plan_metadata(&updated, operator),
                created_at: unix_now().to_string(),
            });
        }
        Ok(updated)
    }

    pub(crate) async fn set_tunnel_plan_enabled(
        &self,
        plan_id: Uuid,
        expected_revision: i64,
        enabled: bool,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let existing = self
            .get_tunnel_plan(plan_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
        let ospf_status = if enabled && existing.plan.ospf.is_some() {
            "unverified"
        } else {
            "disabled"
        };
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let updated = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let plan = plans
                        .iter_mut()
                        .find(|plan| plan.id == plan_id && plan.deleted_at.is_none())
                        .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                    if plan.revision != expected_revision {
                        anyhow::bail!("tunnel_plan_snapshot_stale");
                    }
                    plan.enabled = enabled;
                    plan.revision += 1;
                    reset_ospf_runtime_state(plan, ospf_status);
                    reset_connection_assessment(plan);
                    plan.updated_at = now.clone();
                    plan.clone()
                };
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: if enabled {
                        "network.tunnel_plan_enabled".to_string()
                    } else {
                        "network.tunnel_plan_disabled".to_string()
                    },
                    target: format!("tunnel_plan:{plan_id}"),
                    command_hash: None,
                    metadata: tunnel_plan_enabled_metadata(&updated, operator),
                    created_at: now,
                });
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let result = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET enabled = $2,
                        actor_id = $3,
                        revision = revision + 1,
                        ospf_status = $4,
                        left_ospf_status = $4,
                        right_ospf_status = $4,
                        desired_ospf_cost = NULL,
                        left_current_ospf_cost = NULL,
                        right_current_ospf_cost = NULL,
                        left_ospf_job_id = NULL,
                        right_ospf_job_id = NULL,
                        connection_assessment = 'automatic',
                        connection_assessment_note = NULL,
                        connection_assessed_at = NULL,
                        connection_assessed_by = NULL,
                        updated_at = now()
                    WHERE id = $1 AND deleted_at IS NULL AND revision = $5
                    "#,
                )
                .bind(plan_id)
                .bind(enabled)
                .bind(persisted_actor_id(operator))
                .bind(ospf_status)
                .bind(expected_revision)
                .execute(&mut *tx)
                .await?;
                if result.rows_affected() == 0 {
                    anyhow::bail!("tunnel_plan_snapshot_stale");
                }
                tx.commit().await?;
                let updated = self
                    .get_tunnel_plan(plan_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(persisted_actor_id(operator))
                .bind(if enabled {
                    "network.tunnel_plan_enabled"
                } else {
                    "network.tunnel_plan_disabled"
                })
                .bind(format!("tunnel_plan:{plan_id}"))
                .bind(tunnel_plan_enabled_metadata(&updated, operator))
                .execute(pool)
                .await?;
                Ok(updated)
            }
        }
    }

    pub(crate) async fn update_tunnel_connection_assessment(
        &self,
        plan_id: Uuid,
        expected_revision: i64,
        assessment: &str,
        note: Option<&str>,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let (assessment, note) = normalize_connection_assessment(assessment, note)?;
        match self {
            Self::Memory(memory) => {
                let assessed_at = unix_now().to_string();
                let updated = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let plan = plans
                        .iter_mut()
                        .find(|plan| plan.id == plan_id && plan.deleted_at.is_none())
                        .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                    if plan.revision != expected_revision {
                        anyhow::bail!("tunnel_plan_snapshot_stale");
                    }
                    if !plan.enabled && assessment != "automatic" {
                        anyhow::bail!("tunnel_connection_assessment_requires_enabled_plan");
                    }
                    plan.revision += 1;
                    plan.connection_assessment = assessment.to_string();
                    plan.connection_assessment_note = note.clone();
                    plan.connection_assessed_at =
                        (assessment != "automatic").then(|| assessed_at.clone());
                    plan.connection_assessed_by =
                        (assessment != "automatic").then_some(operator.operator.id);
                    plan.clone()
                };
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "network.tunnel_connection_assessed".to_string(),
                    target: format!("tunnel_plan:{plan_id}"),
                    command_hash: None,
                    metadata: tunnel_connection_assessment_metadata(&updated, operator),
                    created_at: assessed_at,
                });
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let result = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET actor_id = $2,
                        revision = revision + 1,
                        connection_assessment = $3,
                        connection_assessment_note = $4,
                        connection_assessed_at = CASE WHEN $3 = 'automatic' THEN NULL ELSE now() END,
                        connection_assessed_by = CASE WHEN $3 = 'automatic' THEN NULL ELSE $2 END
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND revision = $5
                      AND (enabled = TRUE OR $3 = 'automatic')
                    "#,
                )
                .bind(plan_id)
                .bind(operator.operator.id)
                .bind(&assessment)
                .bind(&note)
                .bind(expected_revision)
                .execute(&mut *tx)
                .await?;
                if result.rows_affected() == 0 {
                    anyhow::bail!("tunnel_plan_snapshot_stale");
                }
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, 'network.tunnel_connection_assessed', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("tunnel_plan:{plan_id}"))
                .bind(serde_json::json!({
                    "assessment": assessment,
                    "note": note,
                    "operator_username": &operator.operator.username,
                    "session_id": operator.session_id,
                }))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                self.get_tunnel_plan(plan_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))
            }
        }
    }

    pub(crate) async fn delete_tunnel_plan(
        &self,
        plan_id: Uuid,
        expected_revision: i64,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let existing = self
            .get_tunnel_plan(plan_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
        if existing.enabled {
            anyhow::bail!("tunnel_plan_disable_before_delete");
        }
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let deleted = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let plan = plans
                        .iter_mut()
                        .find(|plan| plan.id == plan_id && plan.deleted_at.is_none())
                        .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                    if plan.revision != expected_revision {
                        anyhow::bail!("tunnel_plan_snapshot_stale");
                    }
                    if plan.enabled {
                        anyhow::bail!("tunnel_plan_disable_before_delete");
                    }
                    plan.revision += 1;
                    plan.deleted_at = Some(now.clone());
                    plan.deleted_by = persisted_actor_id(operator);
                    plan.deleted_reason = Some("operator_retired".to_string());
                    plan.updated_at = now.clone();
                    plan.clone()
                };
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: persisted_actor_id(operator),
                    action: "network.tunnel_plan_deleted".to_string(),
                    target: format!("tunnel_plan:{plan_id}"),
                    command_hash: None,
                    metadata: tunnel_plan_delete_metadata(&deleted, operator),
                    created_at: now,
                });
                Ok(deleted)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET actor_id = $2,
                        revision = revision + 1,
                        deleted_at = now(),
                        deleted_by = $2,
                        deleted_reason = 'operator_retired',
                        updated_at = now()
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND enabled = FALSE
                      AND revision = $3
                    RETURNING revision, updated_at::text AS updated_at,
                              deleted_at::text AS deleted_at
                    "#,
                )
                .bind(plan_id)
                .bind(persisted_actor_id(operator))
                .bind(expected_revision)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| anyhow::anyhow!("tunnel_plan_snapshot_stale"))?;
                let deleted = TunnelPlanView {
                    revision: row.try_get("revision")?,
                    updated_at: row.try_get("updated_at")?,
                    deleted_at: row.try_get("deleted_at")?,
                    deleted_by: persisted_actor_id(operator),
                    deleted_reason: Some("operator_retired".to_string()),
                    ..existing
                };
                insert_tunnel_audit(
                    &mut tx,
                    operator,
                    "network.tunnel_plan_deleted",
                    &deleted,
                    tunnel_plan_delete_metadata(&deleted, operator),
                )
                .await?;
                tx.commit().await?;
                Ok(deleted)
            }
        }
    }

    pub(crate) async fn stage_tunnel_plan_ospf_jobs(
        &self,
        plan_id: Uuid,
        expected_revision: i64,
        expected_left_cost: Option<u16>,
        expected_right_cost: Option<u16>,
        desired_cost: Option<u16>,
        left_job_id: Uuid,
        right_job_id: Uuid,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let existing = self
            .get_tunnel_plan(plan_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
        validate_ospf_stage(
            &existing,
            expected_revision,
            expected_left_cost,
            expected_right_cost,
            desired_cost,
        )?;
        match self {
            Self::Memory(memory) => {
                let updated = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let plan = plans
                        .iter_mut()
                        .find(|plan| plan.id == plan_id && plan.deleted_at.is_none())
                        .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                    validate_ospf_stage(
                        plan,
                        expected_revision,
                        expected_left_cost,
                        expected_right_cost,
                        desired_cost,
                    )?;
                    set_ospf_pending(plan, desired_cost, left_job_id, right_job_id);
                    plan.updated_at = unix_now().to_string();
                    plan.clone()
                };
                memory.audits.write().await.push(ospf_jobs_audit(
                    &updated,
                    left_job_id,
                    right_job_id,
                    operator,
                ));
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let result = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET actor_id = $2,
                        desired_ospf_cost = $3,
                        ospf_status = 'pending',
                        left_ospf_status = 'pending',
                        right_ospf_status = 'pending',
                        left_ospf_job_id = $4,
                        right_ospf_job_id = $5,
                        updated_at = now()
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND enabled = TRUE
                      AND revision = $8
                      AND left_current_ospf_cost IS NOT DISTINCT FROM $6
                      AND right_current_ospf_cost IS NOT DISTINCT FROM $7
                      AND left_ospf_status <> 'pending'
                      AND right_ospf_status <> 'pending'
                    "#,
                )
                .bind(plan_id)
                .bind(persisted_actor_id(operator))
                .bind(desired_cost.map(i32::from))
                .bind(left_job_id)
                .bind(right_job_id)
                .bind(expected_left_cost.map(i32::from))
                .bind(expected_right_cost.map(i32::from))
                .bind(expected_revision)
                .execute(pool)
                .await?;
                if result.rows_affected() == 0 {
                    anyhow::bail!("tunnel_plan_ospf_snapshot_stale");
                }
                let updated = self
                    .get_tunnel_plan(plan_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, 'network.ospf_jobs_staged', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(persisted_actor_id(operator))
                .bind(format!("tunnel_plan:{plan_id}"))
                .bind(ospf_jobs_metadata(
                    &updated,
                    left_job_id,
                    right_job_id,
                    operator,
                ))
                .execute(pool)
                .await?;
                Ok(updated)
            }
        }
    }

    pub(crate) async fn record_tunnel_plan_ospf_job_result(
        &self,
        plan_id: Uuid,
        side: TunnelEndpointSide,
        job_id: Uuid,
        current_cost: Option<u16>,
        succeeded: bool,
    ) -> Result<Option<TunnelPlanView>> {
        match self {
            Self::Memory(memory) => {
                let mut plans = memory.tunnel_plans.write().await;
                let Some(plan) = plans
                    .iter_mut()
                    .find(|plan| plan.id == plan_id && plan.deleted_at.is_none())
                else {
                    return Ok(None);
                };
                if !ospf_job_matches(plan, side, job_id) {
                    return Ok(None);
                }
                set_endpoint_ospf_result(plan, side, current_cost, succeeded);
                plan.ospf_status = aggregate_ospf_status(plan).to_string();
                plan.updated_at = unix_now().to_string();
                Ok(Some(plan.clone()))
            }
            Self::Postgres(pool) => {
                let (status_column, cost_column, job_column) = match side {
                    TunnelEndpointSide::Left => (
                        "left_ospf_status",
                        "left_current_ospf_cost",
                        "left_ospf_job_id",
                    ),
                    TunnelEndpointSide::Right => (
                        "right_ospf_status",
                        "right_current_ospf_cost",
                        "right_ospf_job_id",
                    ),
                };
                let query = format!(
                    "UPDATE tunnel_plans SET {status_column} = $3, {cost_column} = $4, updated_at = now() \
                     WHERE id = $1 AND deleted_at IS NULL AND {job_column} = $2 \
                       AND {status_column} = 'pending'"
                );
                let result = sqlx::query(&query)
                    .bind(plan_id)
                    .bind(job_id)
                    .bind(if succeeded { "verified" } else { "failed" })
                    .bind(current_cost.map(i32::from))
                    .execute(pool)
                    .await?;
                if result.rows_affected() == 0 {
                    return Ok(None);
                }
                let mut updated = self
                    .get_tunnel_plan(plan_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                updated.ospf_status = aggregate_ospf_status(&updated).to_string();
                sqlx::query(
                    "UPDATE tunnel_plans SET ospf_status = $2, updated_at = now() WHERE id = $1",
                )
                .bind(plan_id)
                .bind(&updated.ospf_status)
                .execute(pool)
                .await?;
                Ok(Some(updated))
            }
        }
    }
}

fn tunnel_plan_from_row(row: sqlx::postgres::PgRow) -> Result<TunnelPlanView, sqlx::Error> {
    let input: SqlJson<TunnelPlanInput> = row.try_get("input")?;
    let plan: SqlJson<TunnelPlan> = row.try_get("plan")?;
    Ok(TunnelPlanView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        kind: parse_tunnel_kind(row.try_get::<String, _>("kind")?.as_str()),
        enabled: row.try_get("enabled")?,
        revision: row.try_get("revision")?,
        left_client_id: row.try_get("left_client_id")?,
        right_client_id: row.try_get("right_client_id")?,
        recommended_ospf_cost: row.try_get("recommended_ospf_cost")?,
        ospf_status: row.try_get("ospf_status")?,
        left_ospf_status: row.try_get("left_ospf_status")?,
        right_ospf_status: row.try_get("right_ospf_status")?,
        desired_ospf_cost: row.try_get("desired_ospf_cost")?,
        left_current_ospf_cost: row.try_get("left_current_ospf_cost")?,
        right_current_ospf_cost: row.try_get("right_current_ospf_cost")?,
        left_ospf_job_id: row.try_get("left_ospf_job_id")?,
        right_ospf_job_id: row.try_get("right_ospf_job_id")?,
        connection_assessment: row.try_get("connection_assessment")?,
        connection_assessment_note: row.try_get("connection_assessment_note")?,
        connection_assessed_at: row.try_get("connection_assessed_at")?,
        connection_assessed_by: row.try_get("connection_assessed_by")?,
        left_runtime_config: untracked_tunnel_runtime_config(
            row.try_get::<String, _>("left_client_id")?.as_str(),
            row.try_get("enabled")?,
        ),
        right_runtime_config: untracked_tunnel_runtime_config(
            row.try_get::<String, _>("right_client_id")?.as_str(),
            row.try_get("enabled")?,
        ),
        input: input.0,
        plan: plan.0,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted_at: row.try_get("deleted_at")?,
        deleted_by: row.try_get("deleted_by")?,
        deleted_reason: row.try_get("deleted_reason")?,
    })
}

fn untracked_tunnel_runtime_config(
    client_id: &str,
    enabled: bool,
) -> TunnelPlanEndpointRuntimeConfigView {
    TunnelPlanEndpointRuntimeConfigView {
        client_id: client_id.to_string(),
        desired: if enabled { "present" } else { "absent" }.to_string(),
        status: "not_dispatched".to_string(),
        cleanup_confirmed: !enabled,
        job_id: None,
        error: None,
        updated_at: None,
    }
}

fn tunnel_endpoint_runtime_config_state(
    plan_id: Uuid,
    client_id: &str,
    enabled: bool,
    state: Option<&RuntimeConfigApplyStateRecord>,
) -> TunnelPlanEndpointRuntimeConfigView {
    let Some(state) = state else {
        return untracked_tunnel_runtime_config(client_id, enabled);
    };
    let desired = if enabled { "present" } else { "absent" };
    if let Some(pending_status) = state.pending_status.as_deref() {
        let pending_matches = state
            .pending_config
            .as_ref()
            .map(|config| runtime_config_contains_tunnel(config, plan_id) == enabled);
        if pending_matches != Some(false) {
            let status = match pending_status {
                "failed" => "failed",
                "queued" => "queued",
                _ => "pending",
            };
            return TunnelPlanEndpointRuntimeConfigView {
                client_id: client_id.to_string(),
                desired: desired.to_string(),
                status: status.to_string(),
                cleanup_confirmed: false,
                job_id: state.pending_job_id,
                error: state.pending_error.clone(),
                updated_at: state.pending_updated_at.clone(),
            };
        }
        return TunnelPlanEndpointRuntimeConfigView {
            client_id: client_id.to_string(),
            desired: desired.to_string(),
            status: "stale_pending".to_string(),
            cleanup_confirmed: false,
            job_id: state.pending_job_id,
            error: state.pending_error.clone(),
            updated_at: state.pending_updated_at.clone(),
        };
    }

    let status = match state.applied_config.as_ref() {
        Some(config) if runtime_config_contains_tunnel(config, plan_id) == enabled => {
            if enabled {
                "applied"
            } else {
                "removed"
            }
        }
        Some(_) if enabled => "not_applied",
        Some(_) => "removal_required",
        None => "not_dispatched",
    };
    TunnelPlanEndpointRuntimeConfigView {
        client_id: client_id.to_string(),
        desired: desired.to_string(),
        status: status.to_string(),
        cleanup_confirmed: !enabled && matches!(status, "removed" | "not_dispatched"),
        job_id: state.applied_job_id,
        error: None,
        updated_at: state.applied_at.clone(),
    }
}

fn runtime_config_contains_tunnel(
    config: &vpsman_common::AgentRuntimeConfig,
    plan_id: Uuid,
) -> bool {
    let plan_id = plan_id.to_string();
    config
        .network
        .runtime_status_telemetry_plans
        .iter()
        .any(|plan| plan.plan_id.as_deref() == Some(plan_id.as_str()))
}

async fn lock_visible_postgres_tunnel_endpoints(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    left_client_id: &str,
    right_client_id: &str,
) -> Result<()> {
    anyhow::ensure!(
        left_client_id != right_client_id,
        "tunnel_plan_endpoints_must_differ"
    );
    lock_postgres_agent_identity_lifecycle(tx).await?;
    let rows = sqlx::query(
        r#"
        SELECT id
        FROM clients
        WHERE id IN ($1, $2)
          AND hidden_at IS NULL
          AND status <> 'deleted'
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(left_client_id)
    .bind(right_client_id)
    .fetch_all(&mut **tx)
    .await?;
    anyhow::ensure!(rows.len() == 2, "tunnel_plan_endpoint_agent_not_found");
    Ok(())
}

fn validate_ospf_stage(
    plan: &TunnelPlanView,
    expected_revision: i64,
    expected_left_cost: Option<u16>,
    expected_right_cost: Option<u16>,
    desired_cost: Option<u16>,
) -> Result<()> {
    if plan.revision != expected_revision {
        anyhow::bail!("tunnel_plan_ospf_snapshot_stale");
    }
    if !plan.enabled {
        anyhow::bail!("tunnel_plan_disabled");
    }
    if plan.plan.ospf.is_none() {
        anyhow::bail!("tunnel_plan_ospf_disabled");
    }
    if plan.left_ospf_status == "pending" || plan.right_ospf_status == "pending" {
        anyhow::bail!("tunnel_plan_ospf_job_in_progress");
    }
    if plan.left_current_ospf_cost != expected_left_cost.map(i32::from)
        || plan.right_current_ospf_cost != expected_right_cost.map(i32::from)
    {
        anyhow::bail!("tunnel_plan_ospf_snapshot_stale");
    }
    if desired_cost == Some(0) {
        anyhow::bail!("tunnel_plan_ospf_cost_invalid");
    }
    Ok(())
}

fn set_ospf_pending(
    plan: &mut TunnelPlanView,
    desired_cost: Option<u16>,
    left_job_id: Uuid,
    right_job_id: Uuid,
) {
    plan.desired_ospf_cost = desired_cost.map(i32::from);
    plan.ospf_status = "pending".to_string();
    plan.left_ospf_status = "pending".to_string();
    plan.right_ospf_status = "pending".to_string();
    plan.left_ospf_job_id = Some(left_job_id);
    plan.right_ospf_job_id = Some(right_job_id);
}

fn reset_ospf_runtime_state(plan: &mut TunnelPlanView, status: &str) {
    plan.ospf_status = status.to_string();
    plan.left_ospf_status = status.to_string();
    plan.right_ospf_status = status.to_string();
    plan.desired_ospf_cost = None;
    plan.left_current_ospf_cost = None;
    plan.right_current_ospf_cost = None;
    plan.left_ospf_job_id = None;
    plan.right_ospf_job_id = None;
}

fn reset_connection_assessment(plan: &mut TunnelPlanView) {
    plan.connection_assessment = "automatic".to_string();
    plan.connection_assessment_note = None;
    plan.connection_assessed_at = None;
    plan.connection_assessed_by = None;
}

fn normalize_connection_assessment(
    assessment: &str,
    note: Option<&str>,
) -> Result<(String, Option<String>)> {
    let assessment = assessment.trim();
    if assessment == "automatic" {
        return Ok((assessment.to_string(), None));
    }
    if !matches!(assessment, "connected" | "disconnected") {
        anyhow::bail!("invalid_tunnel_connection_assessment");
    }
    let note = note.map(str::trim).filter(|value| !value.is_empty());
    if note.is_none_or(|note| note.len() > 500 || note.chars().any(char::is_control)) {
        anyhow::bail!("tunnel_connection_assessment_note_required");
    }
    Ok((assessment.to_string(), note.map(str::to_string)))
}

fn ospf_job_matches(plan: &TunnelPlanView, side: TunnelEndpointSide, job_id: Uuid) -> bool {
    match side {
        TunnelEndpointSide::Left => {
            plan.left_ospf_job_id == Some(job_id) && plan.left_ospf_status == "pending"
        }
        TunnelEndpointSide::Right => {
            plan.right_ospf_job_id == Some(job_id) && plan.right_ospf_status == "pending"
        }
    }
}

fn set_endpoint_ospf_result(
    plan: &mut TunnelPlanView,
    side: TunnelEndpointSide,
    current_cost: Option<u16>,
    succeeded: bool,
) {
    let status = if succeeded { "verified" } else { "failed" }.to_string();
    match side {
        TunnelEndpointSide::Left => {
            plan.left_ospf_status = status;
            plan.left_current_ospf_cost = current_cost.map(i32::from);
        }
        TunnelEndpointSide::Right => {
            plan.right_ospf_status = status;
            plan.right_current_ospf_cost = current_cost.map(i32::from);
        }
    }
}

fn aggregate_ospf_status(plan: &TunnelPlanView) -> &'static str {
    if !plan.enabled || plan.plan.ospf.is_none() {
        return "disabled";
    }
    let left = plan.left_ospf_status.as_str();
    let right = plan.right_ospf_status.as_str();
    if left == "pending" || right == "pending" {
        return "pending";
    }
    if left == "verified" && right == "verified" {
        if let Some(desired) = plan.desired_ospf_cost {
            if plan.left_current_ospf_cost != Some(desired)
                || plan.right_current_ospf_cost != Some(desired)
            {
                return "stale";
            }
        }
        return "verified";
    }
    if (left == "verified" && right == "failed") || (left == "failed" && right == "verified") {
        return "partial";
    }
    if left == "failed" || right == "failed" {
        return "failed";
    }
    if left == "stale" || right == "stale" {
        return "stale";
    }
    "unverified"
}

fn tunnel_plan_metadata(view: &TunnelPlanView, operator: &AuthContext) -> serde_json::Value {
    serde_json::json!({
        "name": &view.name,
        "kind": tunnel_kind_name(view.kind),
        "enabled": view.enabled,
        "left_client_id": &view.left_client_id,
        "right_client_id": &view.right_client_id,
        "runtime_manager": runtime_manager_name(view.plan.runtime_control.manager),
        "runtime_topology_version": &view.plan.runtime_topology.version,
        "ospf_enabled": view.plan.ospf.is_some(),
        "recommended_ospf_cost": view.recommended_ospf_cost,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

fn tunnel_plan_enabled_metadata(
    view: &TunnelPlanView,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "name": &view.name,
        "enabled": view.enabled,
        "left_client_id": &view.left_client_id,
        "right_client_id": &view.right_client_id,
        "ospf_status": &view.ospf_status,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

fn tunnel_connection_assessment_metadata(
    view: &TunnelPlanView,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "name": &view.name,
        "revision": view.revision,
        "assessment": &view.connection_assessment,
        "note": &view.connection_assessment_note,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

fn tunnel_plan_delete_metadata(view: &TunnelPlanView, operator: &AuthContext) -> serde_json::Value {
    serde_json::json!({
        "name": &view.name,
        "revision": view.revision,
        "left_client_id": &view.left_client_id,
        "right_client_id": &view.right_client_id,
        "interface_name": &view.plan.interface_name,
        "deleted_reason": &view.deleted_reason,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

fn ospf_jobs_audit(
    view: &TunnelPlanView,
    left_job_id: Uuid,
    right_job_id: Uuid,
    operator: &AuthContext,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: persisted_actor_id(operator),
        action: "network.ospf_jobs_staged".to_string(),
        target: format!("tunnel_plan:{}", view.id),
        command_hash: None,
        metadata: ospf_jobs_metadata(view, left_job_id, right_job_id, operator),
        created_at: unix_now().to_string(),
    }
}

fn ospf_jobs_metadata(
    view: &TunnelPlanView,
    left_job_id: Uuid,
    right_job_id: Uuid,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "plan_id": view.id,
        "plan_name": &view.name,
        "desired_ospf_cost": view.desired_ospf_cost,
        "left_current_ospf_cost": view.left_current_ospf_cost,
        "right_current_ospf_cost": view.right_current_ospf_cost,
        "left_job_id": left_job_id,
        "right_job_id": right_job_id,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

async fn insert_tunnel_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator: &AuthContext,
    action: &str,
    view: &TunnelPlanView,
    metadata: serde_json::Value,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, $3, $4, NULL, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(operator.operator.id)
    .bind(action)
    .bind(format!("tunnel_plan:{}", view.id))
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn tunnel_kind_name(kind: TunnelKind) -> &'static str {
    match kind {
        TunnelKind::Gre => "gre",
        TunnelKind::Ipip => "ipip",
        TunnelKind::Sit => "sit",
        TunnelKind::Fou => "fou",
        TunnelKind::Openvpn => "openvpn",
        TunnelKind::Wireguard => "wireguard",
        TunnelKind::TunTap => "tun_tap",
        TunnelKind::Custom => "custom",
    }
}

fn parse_tunnel_kind(value: &str) -> TunnelKind {
    match value {
        "gre" => TunnelKind::Gre,
        "ipip" => TunnelKind::Ipip,
        "sit" => TunnelKind::Sit,
        "fou" => TunnelKind::Fou,
        "openvpn" => TunnelKind::Openvpn,
        "wireguard" => TunnelKind::Wireguard,
        "tun_tap" => TunnelKind::TunTap,
        _ => TunnelKind::Custom,
    }
}

fn runtime_manager_name(manager: RuntimeTunnelManager) -> &'static str {
    match manager {
        RuntimeTunnelManager::AgentIproute2Managed => "agent_iproute2_managed",
        RuntimeTunnelManager::ExternalObserved => "external_observed",
        RuntimeTunnelManager::ExternalManagedAdapter => "external_managed_adapter",
    }
}
