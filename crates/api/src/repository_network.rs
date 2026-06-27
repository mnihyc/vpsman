use anyhow::Result;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{RuntimeTunnelManager, TunnelKind, TunnelPlan, TunnelPlanInput};

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
        enabled: bool,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let view = TunnelPlanView {
            id: Uuid::new_v4(),
            name: plan.name.clone(),
            kind: plan.kind,
            enabled,
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
                        enabled = $3,
                        left_client_id = $4,
                        right_client_id = $5,
                        input = $6,
                        plan = $7,
                        recommended_ospf_cost = $8,
                        status = 'planned',
                        left_status = 'planned',
                        right_status = 'planned',
                        last_apply_job_id = NULL,
                        last_rollback_job_id = NULL,
                        updated_at = now()
                    WHERE name = $9 AND deleted_at IS NULL
                    RETURNING
                        id,
                        enabled,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(operator.operator.id)
                .bind(tunnel_kind_name(view.kind))
                .bind(view.enabled)
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

    pub(crate) async fn update_tunnel_plan_ospf_cost(
        &self,
        plan_id: Uuid,
        recommendation_id: &str,
        current_ospf_cost: u16,
        recommended_ospf_cost: u16,
        mutation_intent: &str,
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
                    if plan.recommended_ospf_cost != i32::from(current_ospf_cost) {
                        anyhow::bail!("tunnel_plan_ospf_cost_stale");
                    }
                    plan.recommended_ospf_cost = i32::from(recommended_ospf_cost);
                    plan.plan.recommended_ospf_cost = recommended_ospf_cost;
                    plan.updated_at = now.clone();
                    plan.clone()
                };
                memory
                    .audits
                    .write()
                    .await
                    .push(tunnel_plan_ospf_cost_operator_audit(
                        &updated,
                        recommendation_id,
                        current_ospf_cost,
                        recommended_ospf_cost,
                        mutation_intent,
                        operator,
                        now,
                    ));
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let existing = self
                    .get_tunnel_plan(plan_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                if existing.recommended_ospf_cost != i32::from(current_ospf_cost) {
                    anyhow::bail!("tunnel_plan_ospf_cost_stale");
                }
                let mut next_plan = existing.plan.clone();
                next_plan.recommended_ospf_cost = recommended_ospf_cost;
                let result = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET
                        recommended_ospf_cost = $2,
                        plan = $3,
                        actor_id = $4,
                        updated_at = now()
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND recommended_ospf_cost = $5
                    "#,
                )
                .bind(plan_id)
                .bind(i32::from(recommended_ospf_cost))
                .bind(SqlJson(&next_plan))
                .bind(operator.operator.id)
                .bind(i32::from(current_ospf_cost))
                .execute(pool)
                .await?;
                if result.rows_affected() == 0 {
                    anyhow::bail!("tunnel_plan_ospf_cost_stale");
                }
                let updated = self
                    .get_tunnel_plan(plan_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, 'network.tunnel_plan_ospf_cost_updated', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("tunnel_plan:{plan_id}"))
                .bind(tunnel_plan_ospf_cost_operator_metadata(
                    &updated,
                    recommendation_id,
                    current_ospf_cost,
                    recommended_ospf_cost,
                    mutation_intent,
                    operator,
                ))
                .execute(pool)
                .await?;
                Ok(updated)
            }
        }
    }

    pub(crate) async fn promote_tunnel_plan_to_custom_adapter(
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
                .bind("network.tunnel_plan_promoted_to_custom_adapter")
                .bind(format!("tunnel_plan:{}", persisted.id))
                .bind(tunnel_plan_adapter_metadata(&persisted, existing, operator))
                .execute(pool)
                .await?;
                return Ok(persisted);
            }
        }
        Ok(view)
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
        action: "network.tunnel_plan_promoted_to_custom_adapter".to_string(),
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
        "custom_adapter_status_configured": view.plan.runtime_control.status.is_some(),
        "custom_adapter_startup_configured": view.plan.runtime_control.startup.is_some(),
        "custom_adapter_restart_configured": view.plan.runtime_control.restart.is_some(),
        "custom_adapter_stop_configured": view.plan.runtime_control.stop.is_some(),
        "custom_adapter_cleanup_configured": view.plan.runtime_control.cleanup.is_some(),
        "custom_adapter_traffic_limit_configured": view.plan.runtime_control.traffic_limit_apply.is_some(),
        "runtime_topology_version": &view.plan.runtime_topology.version,
        "mutates_host": view.plan.mutates_host,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

fn tunnel_plan_ospf_cost_operator_audit(
    view: &TunnelPlanView,
    recommendation_id: &str,
    current_ospf_cost: u16,
    recommended_ospf_cost: u16,
    mutation_intent: &str,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "network.tunnel_plan_ospf_cost_updated".to_string(),
        target: format!("tunnel_plan:{}", view.id),
        command_hash: None,
        metadata: tunnel_plan_ospf_cost_operator_metadata(
            view,
            recommendation_id,
            current_ospf_cost,
            recommended_ospf_cost,
            mutation_intent,
            operator,
        ),
        created_at,
    }
}

fn tunnel_plan_ospf_cost_operator_metadata(
    view: &TunnelPlanView,
    recommendation_id: &str,
    current_ospf_cost: u16,
    recommended_ospf_cost: u16,
    mutation_intent: &str,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "recommendation_id": recommendation_id,
        "mutation_intent": mutation_intent,
        "plan_name": &view.name,
        "left_client_id": &view.left_client_id,
        "right_client_id": &view.right_client_id,
        "current_ospf_cost": current_ospf_cost,
        "recommended_ospf_cost": recommended_ospf_cost,
        "result": "updated",
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
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
