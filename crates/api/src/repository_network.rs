use anyhow::Result;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{
    JobCommand, RuntimeTunnelManager, TunnelEndpointSide, TunnelKind, TunnelPlan, TunnelPlanInput,
};

use crate::model::*;
use crate::repository::Repository;
use crate::unix_now;

impl Repository {
    pub(crate) async fn list_tunnel_plans(&self) -> Result<Vec<TunnelPlanView>> {
        match self {
            Self::Memory(memory) => {
                let mut plans: Vec<_> = memory
                    .tunnel_plans
                    .read()
                    .await
                    .iter()
                    .filter(|plan| plan.deleted_at.is_none())
                    .cloned()
                    .collect();
                plans.sort_by(|left, right| right.created_at.cmp(&left.created_at));
                Ok(plans)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        kind,
                        enabled,
                        left_client_id,
                        right_client_id,
                        input,
                        plan,
                        left_status,
                        right_status,
                        recommended_ospf_cost,
                        status,
                        last_apply_job_id,
                        last_rollback_job_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at,
                        deleted_at::text AS deleted_at,
                        deleted_by,
                        deleted_reason
                    FROM tunnel_plans
                    WHERE deleted_at IS NULL
                    ORDER BY updated_at DESC, created_at DESC, id DESC
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let input: SqlJson<TunnelPlanInput> = row.try_get("input")?;
                        let plan: SqlJson<TunnelPlan> = row.try_get("plan")?;
                        Ok(TunnelPlanView {
                            id: row.try_get("id")?,
                            name: row.try_get("name")?,
                            kind: parse_tunnel_kind(row.try_get::<String, _>("kind")?.as_str()),
                            enabled: row.try_get("enabled")?,
                            left_client_id: row.try_get("left_client_id")?,
                            right_client_id: row.try_get("right_client_id")?,
                            left_status: row.try_get("left_status")?,
                            right_status: row.try_get("right_status")?,
                            recommended_ospf_cost: row.try_get("recommended_ospf_cost")?,
                            status: row.try_get("status")?,
                            last_apply_job_id: row.try_get("last_apply_job_id")?,
                            last_rollback_job_id: row.try_get("last_rollback_job_id")?,
                            input: input.0,
                            plan: plan.0,
                            created_at: row.try_get("created_at")?,
                            updated_at: row.try_get("updated_at")?,
                            deleted_at: row.try_get("deleted_at")?,
                            deleted_by: row.try_get("deleted_by")?,
                            deleted_reason: row.try_get("deleted_reason")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn record_tunnel_plan(
        &self,
        input: &TunnelPlanInput,
        plan: &TunnelPlan,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let view = TunnelPlanView {
            id: Uuid::new_v4(),
            name: plan.name.clone(),
            kind: plan.kind,
            enabled: true,
            left_client_id: plan.left_client_id.clone(),
            right_client_id: plan.right_client_id.clone(),
            left_status: "planned".to_string(),
            right_status: "planned".to_string(),
            recommended_ospf_cost: plan.recommended_ospf_cost as i32,
            status: "planned".to_string(),
            last_apply_job_id: None,
            last_rollback_job_id: None,
            input: input.clone(),
            plan: plan.clone(),
            created_at: unix_now().to_string(),
            updated_at: unix_now().to_string(),
            deleted_at: None,
            deleted_by: None,
            deleted_reason: None,
        };
        match self {
            Self::Memory(memory) => {
                let mut plans = memory.tunnel_plans.write().await;
                let persisted = if let Some(existing) = plans
                    .iter_mut()
                    .find(|existing| existing.name == view.name && existing.deleted_at.is_none())
                {
                    let updated = TunnelPlanView {
                        id: existing.id,
                        enabled: existing.enabled,
                        created_at: existing.created_at.clone(),
                        ..view.clone()
                    };
                    *existing = updated.clone();
                    updated
                } else {
                    plans.push(view.clone());
                    view.clone()
                };
                drop(plans);
                memory.audits.write().await.push(tunnel_plan_audit(
                    &persisted,
                    operator,
                    unix_now().to_string(),
                ));
                Ok(persisted)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                    .bind(&view.name)
                    .execute(&mut *tx)
                    .await?;
                let row = if let Some(row) = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET
                        actor_id = $1,
                        kind = $2,
                        left_client_id = $3,
                        right_client_id = $4,
                        input = $5,
                        plan = $6,
                        recommended_ospf_cost = $7,
                        status = 'planned',
                        left_status = 'planned',
                        right_status = 'planned',
                        last_apply_job_id = NULL,
                        last_rollback_job_id = NULL,
                        updated_at = now()
                    WHERE name = $8 AND deleted_at IS NULL
                    RETURNING
                        id,
                        enabled,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(operator.operator.id)
                .bind(tunnel_kind_name(view.kind))
                .bind(&view.left_client_id)
                .bind(&view.right_client_id)
                .bind(SqlJson(input))
                .bind(SqlJson(plan))
                .bind(view.recommended_ospf_cost)
                .bind(&view.name)
                .fetch_optional(&mut *tx)
                .await?
                {
                    row
                } else {
                    sqlx::query(
                        r#"
                        INSERT INTO tunnel_plans (
                            id,
                            actor_id,
                            name,
                            kind,
                            enabled,
                            left_client_id,
                            right_client_id,
                            input,
                            plan,
                            recommended_ospf_cost,
                            status
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'planned')
                        RETURNING
                            id,
                            enabled,
                            created_at::text AS created_at,
                            updated_at::text AS updated_at
                        "#,
                    )
                    .bind(view.id)
                    .bind(operator.operator.id)
                    .bind(&view.name)
                    .bind(tunnel_kind_name(view.kind))
                    .bind(view.enabled)
                    .bind(&view.left_client_id)
                    .bind(&view.right_client_id)
                    .bind(SqlJson(input))
                    .bind(SqlJson(plan))
                    .bind(view.recommended_ospf_cost)
                    .fetch_one(&mut *tx)
                    .await?
                };
                let persisted = TunnelPlanView {
                    id: row.try_get("id")?,
                    enabled: row.try_get("enabled")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                    ..view
                };
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("network.tunnel_plan_created")
                .bind(format!("tunnel_plan:{}", persisted.id))
                .bind(tunnel_plan_metadata(&persisted, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(persisted)
            }
        }
    }

    pub(crate) async fn get_tunnel_plan(&self, id: Uuid) -> Result<Option<TunnelPlanView>> {
        Ok(self
            .list_tunnel_plans()
            .await?
            .into_iter()
            .find(|plan| plan.id == id))
    }

    pub(crate) async fn set_tunnel_plan_enabled(
        &self,
        plan_id: Uuid,
        enabled: bool,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let updated = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let Some(plan) = plans
                        .iter_mut()
                        .find(|plan| plan.id == plan_id && plan.deleted_at.is_none())
                    else {
                        anyhow::bail!("tunnel_plan_not_found");
                    };
                    plan.enabled = enabled;
                    plan.updated_at = now.clone();
                    plan.clone()
                };
                memory
                    .audits
                    .write()
                    .await
                    .push(tunnel_plan_enabled_audit(&updated, enabled, operator, now));
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let result = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET enabled = $2, actor_id = $3, updated_at = now()
                    WHERE id = $1 AND deleted_at IS NULL
                    "#,
                )
                .bind(plan_id)
                .bind(enabled)
                .bind(operator.operator.id)
                .execute(pool)
                .await?;
                if result.rows_affected() == 0 {
                    anyhow::bail!("tunnel_plan_not_found");
                }
                let Some(updated) = self.get_tunnel_plan(plan_id).await? else {
                    anyhow::bail!("tunnel_plan_not_found");
                };
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(if enabled {
                    "network.tunnel_plan_enabled"
                } else {
                    "network.tunnel_plan_disabled"
                })
                .bind(format!("tunnel_plan:{plan_id}"))
                .bind(tunnel_plan_enabled_metadata(&updated, enabled, operator))
                .execute(pool)
                .await?;
                Ok(updated)
            }
        }
    }

    pub(crate) async fn promote_tunnel_plan_to_adapter(
        &self,
        existing: &TunnelPlanView,
        input: &TunnelPlanInput,
        plan: &TunnelPlan,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let view = TunnelPlanView {
            id: existing.id,
            name: plan.name.clone(),
            kind: plan.kind,
            enabled: existing.enabled,
            left_client_id: plan.left_client_id.clone(),
            right_client_id: plan.right_client_id.clone(),
            left_status: "planned".to_string(),
            right_status: "planned".to_string(),
            recommended_ospf_cost: plan.recommended_ospf_cost as i32,
            status: "planned".to_string(),
            last_apply_job_id: None,
            last_rollback_job_id: None,
            input: input.clone(),
            plan: plan.clone(),
            created_at: existing.created_at.clone(),
            updated_at: unix_now().to_string(),
            deleted_at: None,
            deleted_by: None,
            deleted_reason: None,
        };
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut plans = memory.tunnel_plans.write().await;
                if let Some(slot) = plans
                    .iter_mut()
                    .find(|plan| plan.id == existing.id && plan.deleted_at.is_none())
                {
                    *slot = TunnelPlanView {
                        updated_at: now.clone(),
                        ..view.clone()
                    };
                }
                memory
                    .audits
                    .write()
                    .await
                    .push(tunnel_plan_adapter_audit(&view, existing, operator, now));
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET
                        actor_id = $2,
                        name = $3,
                        kind = $4,
                        left_client_id = $5,
                        right_client_id = $6,
                        input = $7,
                        plan = $8,
                        recommended_ospf_cost = $9,
                        status = 'planned',
                        left_status = 'planned',
                        right_status = 'planned',
                        last_apply_job_id = NULL,
                        last_rollback_job_id = NULL,
                        updated_at = now()
                    WHERE id = $1 AND deleted_at IS NULL
                    RETURNING enabled, created_at::text AS created_at, updated_at::text AS updated_at
                    "#,
                )
                .bind(existing.id)
                .bind(operator.operator.id)
                .bind(&view.name)
                .bind(tunnel_kind_name(view.kind))
                .bind(&view.left_client_id)
                .bind(&view.right_client_id)
                .bind(SqlJson(input))
                .bind(SqlJson(plan))
                .bind(view.recommended_ospf_cost)
                .fetch_one(pool)
                .await?;
                let persisted = TunnelPlanView {
                    enabled: row.try_get("enabled")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                    ..view
                };
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("network.tunnel_plan_promoted_to_adapter")
                .bind(format!("tunnel_plan:{}", persisted.id))
                .bind(tunnel_plan_adapter_metadata(&persisted, existing, operator))
                .execute(pool)
                .await?;
                return Ok(persisted);
            }
        }
        Ok(view)
    }

    pub(crate) async fn record_tunnel_plan_execution(
        &self,
        job_id: Uuid,
        operation: &JobCommand,
        job_status: &str,
    ) -> Result<()> {
        self.record_tunnel_plan_execution_with_mode(
            job_id,
            operation,
            job_status,
            TunnelPlanExecutionRecordMode::Normal,
        )
        .await
    }

    pub(crate) async fn repair_tunnel_plan_execution(
        &self,
        job_id: Uuid,
        operation: &JobCommand,
        job_status: &str,
    ) -> Result<()> {
        self.record_tunnel_plan_execution_with_mode(
            job_id,
            operation,
            job_status,
            TunnelPlanExecutionRecordMode::Repair,
        )
        .await
    }

    async fn record_tunnel_plan_execution_with_mode(
        &self,
        job_id: Uuid,
        operation: &JobCommand,
        job_status: &str,
        mode: TunnelPlanExecutionRecordMode,
    ) -> Result<()> {
        if job_status != "completed" {
            return Ok(());
        }
        if let Some(update) = tunnel_plan_ospf_cost_update(job_id, operation) {
            self.record_tunnel_plan_ospf_cost_update(&update).await?;
            return Ok(());
        }
        let Some(update) = tunnel_plan_status_update(job_id, operation) else {
            return Ok(());
        };
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let aggregate_status = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let Some(plan) = plans
                        .iter_mut()
                        .find(|plan| plan.name == update.plan_name && plan.deleted_at.is_none())
                    else {
                        return Ok(());
                    };
                    if mode == TunnelPlanExecutionRecordMode::Repair
                        && !repair_can_record_tunnel_execution(plan, &update, job_id)
                    {
                        return Ok(());
                    }
                    match update.side {
                        TunnelEndpointSide::Left => {
                            plan.left_status = update.endpoint_status.to_string()
                        }
                        TunnelEndpointSide::Right => {
                            plan.right_status = update.endpoint_status.to_string()
                        }
                    }
                    match update.kind {
                        TunnelPlanExecutionKind::Apply => plan.last_apply_job_id = Some(job_id),
                        TunnelPlanExecutionKind::Rollback => {
                            plan.last_rollback_job_id = Some(job_id)
                        }
                    }
                    plan.status = aggregate_tunnel_plan_status(
                        &plan.left_status,
                        &plan.right_status,
                        update.endpoint_status,
                        update.complete_status,
                        update.partial_status,
                    )
                    .to_string();
                    plan.updated_at = now.clone();
                    (plan.id, plan.status.clone())
                };
                let mut audits = memory.audits.write().await;
                let job_id_string = job_id.to_string();
                if !audits.iter().any(|audit| {
                    audit.action == update.audit_action
                        && audit.metadata["job_id"].as_str() == Some(job_id_string.as_str())
                }) {
                    audits.push(tunnel_plan_state_audit(
                        aggregate_status.0,
                        &update,
                        aggregate_status.1.as_str(),
                        now,
                    ));
                }
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET
                        left_status = CASE WHEN $2 = 'left' THEN $3 ELSE left_status END,
                        right_status = CASE WHEN $2 = 'right' THEN $3 ELSE right_status END,
                        status = CASE
                            WHEN
                                (CASE WHEN $2 = 'left' THEN $3 ELSE left_status END) = $3
                                AND (CASE WHEN $2 = 'right' THEN $3 ELSE right_status END) = $3
                                THEN $4
                            ELSE $5
                        END,
                        last_apply_job_id = COALESCE($6, last_apply_job_id),
                        last_rollback_job_id = COALESCE($7, last_rollback_job_id),
                        updated_at = now()
                    WHERE name = $1 AND deleted_at IS NULL
                      AND (
                          NOT $8
                          OR ($6::uuid IS NOT NULL AND (last_apply_job_id IS NULL OR last_apply_job_id = $6))
                          OR ($7::uuid IS NOT NULL AND (last_rollback_job_id IS NULL OR last_rollback_job_id = $7))
                      )
                    RETURNING id, status
                    "#,
                )
                .bind(update.plan_name.as_str())
                .bind(side_name(update.side))
                .bind(update.endpoint_status)
                .bind(update.complete_status)
                .bind(update.partial_status)
                .bind(match update.kind {
                    TunnelPlanExecutionKind::Apply => Some(job_id),
                    TunnelPlanExecutionKind::Rollback => None,
                })
                .bind(match update.kind {
                    TunnelPlanExecutionKind::Apply => None,
                    TunnelPlanExecutionKind::Rollback => Some(job_id),
                })
                .bind(mode == TunnelPlanExecutionRecordMode::Repair)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(());
                };
                let plan_id: Uuid = row.try_get("id")?;
                let status: String = row.try_get("status")?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    SELECT $1, NULL, $2, $3, NULL, $4
                    WHERE NOT EXISTS (
                        SELECT 1
                        FROM audit_logs
                        WHERE action = $2
                          AND target = $3
                          AND metadata->>'job_id' = $5
                    )
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(update.audit_action)
                .bind(format!("tunnel_plan:{plan_id}"))
                .bind(tunnel_plan_state_metadata(&update, status.as_str()))
                .bind(job_id.to_string())
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    async fn record_tunnel_plan_ospf_cost_update(
        &self,
        update: &TunnelPlanOspfCostUpdate,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let Some((plan_id, status, result)) = ({
                    let mut plans = memory.tunnel_plans.write().await;
                    let Some(plan) = plans
                        .iter_mut()
                        .find(|plan| plan.name == update.plan_name && plan.deleted_at.is_none())
                    else {
                        return Ok(());
                    };
                    let result = if plan.recommended_ospf_cost
                        == i32::from(update.recommended_ospf_cost)
                    {
                        "idempotent"
                    } else if plan.recommended_ospf_cost == i32::from(update.current_ospf_cost) {
                        plan.recommended_ospf_cost = i32::from(update.recommended_ospf_cost);
                        plan.plan = update.plan.clone();
                        plan.updated_at = now.clone();
                        "updated"
                    } else {
                        "stale_ignored"
                    };
                    Some((plan.id, plan.status.clone(), result))
                }) else {
                    return Ok(());
                };
                let mut audits = memory.audits.write().await;
                let job_id_string = update.job_id.to_string();
                if !audits.iter().any(|audit| {
                    audit.action == "network.tunnel_plan_ospf_cost_updated"
                        && audit.metadata["job_id"].as_str() == Some(job_id_string.as_str())
                }) {
                    audits.push(tunnel_plan_ospf_cost_audit(
                        plan_id,
                        update,
                        status.as_str(),
                        result,
                        now,
                    ));
                }
            }
            Self::Postgres(pool) => {
                let updated = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET
                        recommended_ospf_cost = $2,
                        plan = $3,
                        updated_at = now()
                    WHERE name = $1
                      AND deleted_at IS NULL
                      AND recommended_ospf_cost = $4
                    RETURNING id, status
                    "#,
                )
                .bind(update.plan_name.as_str())
                .bind(i32::from(update.recommended_ospf_cost))
                .bind(SqlJson(&update.plan))
                .bind(i32::from(update.current_ospf_cost))
                .fetch_optional(pool)
                .await?;
                let (plan_id, status, result) = if let Some(row) = updated {
                    (
                        row.try_get::<Uuid, _>("id")?,
                        row.try_get::<String, _>("status")?,
                        "updated",
                    )
                } else {
                    let Some(row) = sqlx::query(
                        r#"
                        SELECT id, status, recommended_ospf_cost
                        FROM tunnel_plans
                        WHERE name = $1 AND deleted_at IS NULL
                        "#,
                    )
                    .bind(update.plan_name.as_str())
                    .fetch_optional(pool)
                    .await?
                    else {
                        return Ok(());
                    };
                    let current: i32 = row.try_get("recommended_ospf_cost")?;
                    let result = if current == i32::from(update.recommended_ospf_cost) {
                        "idempotent"
                    } else {
                        "stale_ignored"
                    };
                    (
                        row.try_get::<Uuid, _>("id")?,
                        row.try_get::<String, _>("status")?,
                        result,
                    )
                };
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    SELECT $1, NULL, 'network.tunnel_plan_ospf_cost_updated', $2, NULL, $3
                    WHERE NOT EXISTS (
                        SELECT 1
                        FROM audit_logs
                        WHERE action = 'network.tunnel_plan_ospf_cost_updated'
                          AND target = $2
                          AND metadata->>'job_id' = $4
                    )
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(format!("tunnel_plan:{plan_id}"))
                .bind(tunnel_plan_ospf_cost_metadata(
                    update,
                    status.as_str(),
                    result,
                ))
                .bind(update.job_id.to_string())
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_tunnel_plan_promotion_audit(
        &self,
        view: &TunnelPlanView,
        operator: &AuthContext,
        tunnel: &TelemetryTunnelView,
    ) -> Result<()> {
        let metadata = serde_json::json!({
            "plan_id": view.id,
            "plan_name": &view.name,
            "client_id": &tunnel.client_id,
            "interface": &tunnel.interface,
            "kind": &tunnel.kind,
            "observed_at": &tunnel.observed_at,
            "mutation_policy": &tunnel.mutation_policy,
            "promotion_required": tunnel.promotion_required,
            "runtime_manager": runtime_manager_name(view.plan.runtime_control.manager),
            "operator_username": &operator.operator.username,
            "session_id": operator.session_id,
        });
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "network.tunnel_plan_promoted_from_telemetry".to_string(),
                    target: format!("tunnel_plan:{}", view.id),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("network.tunnel_plan_promoted_from_telemetry")
                .bind(format!("tunnel_plan:{}", view.id))
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

fn tunnel_plan_audit(
    view: &TunnelPlanView,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "network.tunnel_plan_created".to_string(),
        target: format!("tunnel_plan:{}", view.id),
        command_hash: None,
        metadata: tunnel_plan_metadata(view, operator),
        created_at,
    }
}

fn tunnel_plan_metadata(view: &TunnelPlanView, operator: &AuthContext) -> serde_json::Value {
    serde_json::json!({
        "name": &view.name,
        "kind": tunnel_kind_name(view.kind),
        "enabled": view.enabled,
        "left_client_id": &view.left_client_id,
        "right_client_id": &view.right_client_id,
        "recommended_ospf_cost": view.recommended_ospf_cost,
        "runtime_manager": runtime_manager_name(view.plan.runtime_control.manager),
        "runtime_topology_version": &view.plan.runtime_topology.version,
        "mutates_host": view.plan.mutates_host,
        "touched_files": &view.plan.touched_files,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

fn tunnel_plan_enabled_audit(
    view: &TunnelPlanView,
    enabled: bool,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: if enabled {
            "network.tunnel_plan_enabled".to_string()
        } else {
            "network.tunnel_plan_disabled".to_string()
        },
        target: format!("tunnel_plan:{}", view.id),
        command_hash: None,
        metadata: tunnel_plan_enabled_metadata(view, enabled, operator),
        created_at,
    }
}

fn tunnel_plan_enabled_metadata(
    view: &TunnelPlanView,
    enabled: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "name": &view.name,
        "enabled": enabled,
        "left_client_id": &view.left_client_id,
        "right_client_id": &view.right_client_id,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

fn tunnel_plan_adapter_audit(
    view: &TunnelPlanView,
    previous: &TunnelPlanView,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "network.tunnel_plan_promoted_to_adapter".to_string(),
        target: format!("tunnel_plan:{}", view.id),
        command_hash: None,
        metadata: tunnel_plan_adapter_metadata(view, previous, operator),
        created_at,
    }
}

fn tunnel_plan_adapter_metadata(
    view: &TunnelPlanView,
    previous: &TunnelPlanView,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "name": &view.name,
        "previous_runtime_manager": runtime_manager_name(previous.plan.runtime_control.manager),
        "runtime_manager": runtime_manager_name(view.plan.runtime_control.manager),
        "kind": tunnel_kind_name(view.kind),
        "left_client_id": &view.left_client_id,
        "right_client_id": &view.right_client_id,
        "adapter_status_configured": view.plan.runtime_control.status.is_some(),
        "adapter_startup_configured": view.plan.runtime_control.startup.is_some(),
        "adapter_restart_configured": view.plan.runtime_control.restart.is_some(),
        "adapter_stop_configured": view.plan.runtime_control.stop.is_some(),
        "adapter_cleanup_configured": view.plan.runtime_control.cleanup.is_some(),
        "adapter_traffic_limit_configured": view.plan.runtime_control.traffic_limit_apply.is_some(),
        "runtime_topology_version": &view.plan.runtime_topology.version,
        "mutates_host": view.plan.mutates_host,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

#[derive(Clone, Copy)]
enum TunnelPlanExecutionKind {
    Apply,
    Rollback,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TunnelPlanExecutionRecordMode {
    Normal,
    Repair,
}

struct TunnelPlanStatusUpdate {
    kind: TunnelPlanExecutionKind,
    plan_name: String,
    side: TunnelEndpointSide,
    endpoint_status: &'static str,
    complete_status: &'static str,
    partial_status: &'static str,
    audit_action: &'static str,
    job_id: Uuid,
}

struct TunnelPlanOspfCostUpdate {
    plan_name: String,
    side: TunnelEndpointSide,
    plan: TunnelPlan,
    current_ospf_cost: u16,
    recommended_ospf_cost: u16,
    job_id: Uuid,
}

fn repair_can_record_tunnel_execution(
    plan: &TunnelPlanView,
    update: &TunnelPlanStatusUpdate,
    job_id: Uuid,
) -> bool {
    match update.kind {
        TunnelPlanExecutionKind::Apply => plan
            .last_apply_job_id
            .is_none_or(|last_job_id| last_job_id == job_id),
        TunnelPlanExecutionKind::Rollback => plan
            .last_rollback_job_id
            .is_none_or(|last_job_id| last_job_id == job_id),
    }
}

fn tunnel_plan_ospf_cost_update(
    job_id: Uuid,
    operation: &JobCommand,
) -> Option<TunnelPlanOspfCostUpdate> {
    match operation {
        JobCommand::NetworkOspfCostUpdate {
            plan,
            side,
            current_ospf_cost,
            recommended_ospf_cost,
            ..
        } => Some(TunnelPlanOspfCostUpdate {
            plan_name: plan.name.clone(),
            side: *side,
            plan: (**plan).clone(),
            current_ospf_cost: *current_ospf_cost,
            recommended_ospf_cost: *recommended_ospf_cost,
            job_id,
        }),
        _ => None,
    }
}

fn tunnel_plan_status_update(
    job_id: Uuid,
    operation: &JobCommand,
) -> Option<TunnelPlanStatusUpdate> {
    match operation {
        JobCommand::NetworkApply { plan, side, .. } => Some(TunnelPlanStatusUpdate {
            kind: TunnelPlanExecutionKind::Apply,
            plan_name: plan.name.clone(),
            side: *side,
            endpoint_status: "applied",
            complete_status: "applied",
            partial_status: "partially_applied",
            audit_action: "network.tunnel_plan_applied",
            job_id,
        }),
        JobCommand::NetworkRollback { plan, side } => Some(TunnelPlanStatusUpdate {
            kind: TunnelPlanExecutionKind::Rollback,
            plan_name: plan.name.clone(),
            side: *side,
            endpoint_status: "rolled_back",
            complete_status: "rolled_back",
            partial_status: "partially_rolled_back",
            audit_action: "network.tunnel_plan_rolled_back",
            job_id,
        }),
        _ => None,
    }
}

fn tunnel_plan_ospf_cost_audit(
    plan_id: Uuid,
    update: &TunnelPlanOspfCostUpdate,
    aggregate_status: &str,
    result: &str,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: None,
        action: "network.tunnel_plan_ospf_cost_updated".to_string(),
        target: format!("tunnel_plan:{plan_id}"),
        command_hash: None,
        metadata: tunnel_plan_ospf_cost_metadata(update, aggregate_status, result),
        created_at,
    }
}

fn tunnel_plan_ospf_cost_metadata(
    update: &TunnelPlanOspfCostUpdate,
    aggregate_status: &str,
    result: &str,
) -> serde_json::Value {
    serde_json::json!({
        "job_id": update.job_id,
        "plan_name": &update.plan_name,
        "side": side_name(update.side),
        "current_ospf_cost": update.current_ospf_cost,
        "recommended_ospf_cost": update.recommended_ospf_cost,
        "aggregate_status": aggregate_status,
        "result": result,
    })
}

fn aggregate_tunnel_plan_status<'a>(
    left_status: &str,
    right_status: &str,
    endpoint_status: &str,
    complete_status: &'a str,
    partial_status: &'a str,
) -> &'a str {
    if left_status == endpoint_status && right_status == endpoint_status {
        complete_status
    } else {
        partial_status
    }
}

fn tunnel_plan_state_audit(
    plan_id: Uuid,
    update: &TunnelPlanStatusUpdate,
    aggregate_status: &str,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: None,
        action: update.audit_action.to_string(),
        target: format!("tunnel_plan:{plan_id}"),
        command_hash: None,
        metadata: tunnel_plan_state_metadata(update, aggregate_status),
        created_at,
    }
}

fn tunnel_plan_state_metadata(
    update: &TunnelPlanStatusUpdate,
    aggregate_status: &str,
) -> serde_json::Value {
    serde_json::json!({
        "job_id": update.job_id,
        "plan_name": update.plan_name,
        "side": side_name(update.side),
        "endpoint_status": update.endpoint_status,
        "status": aggregate_status,
    })
}

fn side_name(side: TunnelEndpointSide) -> &'static str {
    match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    }
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
        "custom" => TunnelKind::Custom,
        _ => TunnelKind::Gre,
    }
}

fn runtime_manager_name(manager: RuntimeTunnelManager) -> &'static str {
    match manager {
        RuntimeTunnelManager::AgentIproute2Managed => "agent_iproute2_managed",
        RuntimeTunnelManager::ExternalObserved => "external_observed",
        RuntimeTunnelManager::ExternalManagedAdapter => "external_managed_adapter",
    }
}
