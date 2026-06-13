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
                let mut plans = memory.tunnel_plans.read().await.clone();
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
                        updated_at::text AS updated_at
                    FROM tunnel_plans
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
        };
        match self {
            Self::Memory(memory) => {
                let mut plans = memory.tunnel_plans.write().await;
                if let Some(existing) = plans.iter_mut().find(|existing| existing.name == view.name)
                {
                    *existing = view.clone();
                } else {
                    plans.push(view.clone());
                }
                memory.audits.write().await.push(tunnel_plan_audit(
                    &view,
                    operator,
                    unix_now().to_string(),
                ));
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    INSERT INTO tunnel_plans (
                        id,
                        actor_id,
                        name,
                        kind,
                        left_client_id,
                        right_client_id,
                        input,
                        plan,
                        recommended_ospf_cost,
                        status
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'planned')
                    ON CONFLICT (name) DO UPDATE SET
                        actor_id = EXCLUDED.actor_id,
                        kind = EXCLUDED.kind,
                        left_client_id = EXCLUDED.left_client_id,
                        right_client_id = EXCLUDED.right_client_id,
                        input = EXCLUDED.input,
                        plan = EXCLUDED.plan,
                        recommended_ospf_cost = EXCLUDED.recommended_ospf_cost,
                        status = 'planned',
                        left_status = 'planned',
                        right_status = 'planned',
                        last_apply_job_id = NULL,
                        last_rollback_job_id = NULL,
                        updated_at = now()
                    RETURNING
                        id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(view.id)
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
                    id: row.try_get("id")?,
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
                .execute(pool)
                .await?;
                return Ok(persisted);
            }
        }
        Ok(view)
    }

    pub(crate) async fn get_tunnel_plan(&self, id: Uuid) -> Result<Option<TunnelPlanView>> {
        Ok(self
            .list_tunnel_plans()
            .await?
            .into_iter()
            .find(|plan| plan.id == id))
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
        };
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut plans = memory.tunnel_plans.write().await;
                if let Some(slot) = plans.iter_mut().find(|plan| plan.id == existing.id) {
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
                    WHERE id = $1
                    RETURNING created_at::text AS created_at, updated_at::text AS updated_at
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
        if job_status != "completed" {
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
                    let Some(plan) = plans.iter_mut().find(|plan| plan.name == update.plan_name)
                    else {
                        return Ok(());
                    };
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
                memory.audits.write().await.push(tunnel_plan_state_audit(
                    aggregate_status.0,
                    &update,
                    aggregate_status.1.as_str(),
                    now,
                ));
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
                    WHERE name = $1
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
                    VALUES ($1, NULL, $2, $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(update.audit_action)
                .bind(format!("tunnel_plan:{plan_id}"))
                .bind(tunnel_plan_state_metadata(&update, status.as_str()))
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
