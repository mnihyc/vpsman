use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
};

use anyhow::{Context, Result};
use sqlx::{types::Json as SqlJson, Row};
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    tunnel_runtime_evidence_identity_hash, tunnel_topology_identity_hash, RuntimeTunnelManager,
    TunnelBuiltinCredentials, TunnelEndpointSide, TunnelKind, TunnelPlan, TunnelPlanInput,
};

use crate::{
    internal_operator::persisted_actor_id,
    model::*,
    repository::Repository,
    repository_key_lifecycle::{
        lock_postgres_agent_identity_lifecycle, require_visible_memory_clients,
        require_visible_postgres_clients_in_tx,
    },
    repository_network_observations::deactivate_postgres_automatic_observation_series_for_plan,
    repository_operational_alerts::reconcile_postgres_tunnel_alerts_for_clients_in_tx,
    repository_tunnel_credentials::{
        generate_tunnel_builtin_credentials, next_credential_generation,
        reconcile_tunnel_builtin_credentials,
    },
    unix_now,
};

#[derive(Clone, Copy)]
enum ControllerTunnelPlanPage {
    Automatic,
    Pending,
}

const TUNNEL_PLAN_MANAGEMENT_READ_LIMIT: usize = 1_000;

enum TunnelPlanRead {
    Plan(Box<TunnelPlanView>),
    Corrupt(Box<TunnelPlanCorruptView>),
}

pub(crate) struct TunnelPlanRecordAttempt {
    pub(crate) plan_id: Uuid,
    pub(crate) plan: Result<TunnelPlanView>,
}

pub(crate) struct TunnelPlanIdentity {
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) revision: i64,
    pub(crate) left_client_id: String,
    pub(crate) right_client_id: String,
}

impl Repository {
    pub(crate) async fn list_tunnel_plans(&self) -> Result<Vec<TunnelPlanView>> {
        let reads = self
            .list_tunnel_plan_reads(TUNNEL_PLAN_MANAGEMENT_READ_LIMIT)
            .await?;
        let mut plans = Vec::with_capacity(reads.len());
        for read in reads {
            match read {
                TunnelPlanRead::Plan(plan) => plans.push(*plan),
                TunnelPlanRead::Corrupt(corrupt) => warn!(
                    event = "tunnel_plan_configuration_corrupt",
                    plan_id = %corrupt.id,
                    error = %corrupt.configuration_error,
                    "isolated malformed persisted tunnel plan"
                ),
            }
        }
        Ok(plans)
    }

    pub(crate) async fn list_tunnel_plan_items(&self) -> Result<Vec<TunnelPlanListItem>> {
        let reads = self
            .list_tunnel_plan_reads(TUNNEL_PLAN_MANAGEMENT_READ_LIMIT)
            .await?;
        Ok(reads
            .into_iter()
            .map(|read| match read {
                TunnelPlanRead::Plan(plan) => TunnelPlanListItem::Plan(plan),
                TunnelPlanRead::Corrupt(corrupt) => {
                    warn!(
                        event = "tunnel_plan_configuration_corrupt",
                        plan_id = %corrupt.id,
                        error = %corrupt.configuration_error,
                        "exposing malformed persisted tunnel plan to management"
                    );
                    TunnelPlanListItem::Corrupt(corrupt)
                }
            })
            .collect())
    }

    async fn list_tunnel_plan_reads(&self, limit: usize) -> Result<Vec<TunnelPlanRead>> {
        let limit = limit.max(1);
        let mut reads = match self {
            Self::Memory(memory) => {
                let mut plans = memory
                    .tunnel_plans
                    .read()
                    .await
                    .iter()
                    .filter(|plan| plan.deleted_at.is_none())
                    .cloned()
                    .collect::<Vec<_>>();
                plans.sort_by(|left, right| {
                    right
                        .updated_at
                        .cmp(&left.updated_at)
                        .then_with(|| right.created_at.cmp(&left.created_at))
                        .then_with(|| right.id.cmp(&left.id))
                });
                plans.truncate(limit);
                plans
                    .into_iter()
                    .map(|plan| TunnelPlanRead::Plan(Box::new(plan)))
                    .collect()
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id, name, kind, enabled, revision, left_client_id, right_client_id,
                        input, plan, builtin_credentials, recommended_ospf_cost,
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
                    LIMIT $1
                    "#,
                )
                .bind(limit as i64)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| match tunnel_plan_from_row(&row) {
                        Ok(plan) => Ok(TunnelPlanRead::Plan(Box::new(plan))),
                        Err(error) => Ok(TunnelPlanRead::Corrupt(Box::new(
                            tunnel_plan_corrupt_from_row(&row, &error)?,
                        ))),
                    })
                    .collect::<Result<Vec<_>>>()?
            }
        };
        let apply_states = self
            .list_runtime_config_apply_records(None)
            .await?
            .into_iter()
            .map(|state| (state.client_id.clone(), state))
            .collect::<HashMap<_, _>>();
        for read in &mut reads {
            if let TunnelPlanRead::Plan(plan) = read {
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
        }
        Ok(reads)
    }

    pub(crate) async fn list_automatic_tunnel_plan_ids_for_controller(
        &self,
        limit: usize,
    ) -> Result<Vec<Uuid>> {
        self.list_controller_tunnel_plan_ids_by_scan(ControllerTunnelPlanPage::Automatic, limit)
            .await
    }

    pub(crate) async fn list_pending_tunnel_plan_ids_for_reconciliation(
        &self,
        limit: usize,
    ) -> Result<Vec<Uuid>> {
        self.list_controller_tunnel_plan_ids_by_scan(ControllerTunnelPlanPage::Pending, limit)
            .await
    }

    async fn list_controller_tunnel_plan_ids_by_scan(
        &self,
        page: ControllerTunnelPlanPage,
        limit: usize,
    ) -> Result<Vec<Uuid>> {
        let limit = limit.max(1);
        match self {
            Self::Memory(memory) => {
                let plans = memory.tunnel_plans.read().await;
                let scans = match page {
                    ControllerTunnelPlanPage::Automatic => {
                        memory.automatic_ospf_plan_scans.read().await
                    }
                    ControllerTunnelPlanPage::Pending => {
                        memory.pending_ospf_plan_reconciliations.read().await
                    }
                };
                let mut plan_ids = plans
                    .iter()
                    .filter(|plan| {
                        plan.deleted_at.is_none()
                            && match page {
                                ControllerTunnelPlanPage::Automatic => {
                                    plan.enabled
                                        && plan.plan.ospf.as_ref().is_some_and(|ospf| {
                                            ospf.mode == vpsman_common::OspfControlMode::Automatic
                                        })
                                }
                                ControllerTunnelPlanPage::Pending => plan.ospf_status == "pending",
                            }
                    })
                    .map(|plan| plan.id)
                    .collect::<Vec<_>>();
                plan_ids.sort_by_key(|plan_id| (scans.get(plan_id).copied(), *plan_id));
                plan_ids.truncate(limit);
                Ok(plan_ids)
            }
            Self::Postgres(pool) => {
                let (predicate, scan_column) = match page {
                    ControllerTunnelPlanPage::Automatic => (
                        "enabled = TRUE AND plan->'ospf'->>'mode' = 'automatic'",
                        "automatic_ospf_scanned_at",
                    ),
                    ControllerTunnelPlanPage::Pending => {
                        ("ospf_status = 'pending'", "pending_ospf_reconciled_at")
                    }
                };
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT id
                    FROM tunnel_plans
                    WHERE deleted_at IS NULL
                      AND {predicate}
                    ORDER BY {scan_column} ASC NULLS FIRST, id
                    LIMIT $1
                    "#
                ))
                .bind(limit as i64)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| row.try_get("id").map_err(Into::into))
                    .collect()
            }
        }
    }

    pub(crate) async fn tunnel_plan_record_attempts(
        &self,
        plan_ids: &[Uuid],
    ) -> Result<Vec<TunnelPlanRecordAttempt>> {
        if plan_ids.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let plans = memory.tunnel_plans.read().await;
                Ok(plan_ids
                    .iter()
                    .filter_map(|plan_id| {
                        plans
                            .iter()
                            .find(|plan| plan.id == *plan_id && plan.deleted_at.is_none())
                            .cloned()
                            .map(|plan| TunnelPlanRecordAttempt {
                                plan_id: *plan_id,
                                plan: Ok(plan),
                            })
                    })
                    .collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id, name, kind, enabled, revision, left_client_id, right_client_id,
                        input, plan, builtin_credentials, recommended_ospf_cost,
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
                    WHERE id = ANY($1::uuid[])
                      AND deleted_at IS NULL
                    "#,
                )
                .bind(plan_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let plan_id = row.try_get("id")?;
                        Ok(TunnelPlanRecordAttempt {
                            plan_id,
                            plan: tunnel_plan_from_row(&row),
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn mark_automatic_tunnel_plans_scanned(
        &self,
        plan_ids: &[Uuid],
    ) -> Result<()> {
        self.mark_controller_tunnel_plans_scanned(ControllerTunnelPlanPage::Automatic, plan_ids)
            .await
    }

    pub(crate) async fn mark_pending_tunnel_plans_reconciled(
        &self,
        plan_ids: &[Uuid],
    ) -> Result<()> {
        self.mark_controller_tunnel_plans_scanned(ControllerTunnelPlanPage::Pending, plan_ids)
            .await
    }

    async fn mark_controller_tunnel_plans_scanned(
        &self,
        page: ControllerTunnelPlanPage,
        plan_ids: &[Uuid],
    ) -> Result<()> {
        if plan_ids.is_empty() {
            return Ok(());
        }
        match self {
            Self::Memory(memory) => {
                let mut scans = match page {
                    ControllerTunnelPlanPage::Automatic => {
                        memory.automatic_ospf_plan_scans.write().await
                    }
                    ControllerTunnelPlanPage::Pending => {
                        memory.pending_ospf_plan_reconciliations.write().await
                    }
                };
                let generation = scans
                    .values()
                    .copied()
                    .max()
                    .unwrap_or_default()
                    .saturating_add(1);
                for plan_id in plan_ids {
                    scans.insert(*plan_id, generation);
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                let scan_column = match page {
                    ControllerTunnelPlanPage::Automatic => "automatic_ospf_scanned_at",
                    ControllerTunnelPlanPage::Pending => "pending_ospf_reconciled_at",
                };
                sqlx::query(&format!(
                    "UPDATE tunnel_plans SET {scan_column} = now() WHERE id = ANY($1::uuid[])"
                ))
                .bind(plan_ids)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn get_tunnel_plan(&self, id: Uuid) -> Result<Option<TunnelPlanView>> {
        let mut plan = self.get_tunnel_plan_record(id).await?;
        let Some(plan) = plan.as_mut() else {
            return Ok(None);
        };
        let left_state = self
            .list_runtime_config_apply_records(Some(&plan.left_client_id))
            .await?
            .into_iter()
            .next();
        let right_state = if plan.right_client_id == plan.left_client_id {
            left_state.clone()
        } else {
            self.list_runtime_config_apply_records(Some(&plan.right_client_id))
                .await?
                .into_iter()
                .next()
        };
        plan.left_runtime_config = tunnel_endpoint_runtime_config_state(
            plan.id,
            &plan.left_client_id,
            plan.enabled,
            left_state.as_ref(),
        );
        plan.right_runtime_config = tunnel_endpoint_runtime_config_state(
            plan.id,
            &plan.right_client_id,
            plan.enabled,
            right_state.as_ref(),
        );
        Ok(Some(plan.clone()))
    }

    pub(crate) async fn get_tunnel_plan_identity(
        &self,
        id: Uuid,
    ) -> Result<Option<TunnelPlanIdentity>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .tunnel_plans
                .read()
                .await
                .iter()
                .find(|plan| plan.id == id && plan.deleted_at.is_none())
                .map(|plan| TunnelPlanIdentity {
                    name: plan.name.clone(),
                    enabled: plan.enabled,
                    revision: plan.revision,
                    left_client_id: plan.left_client_id.clone(),
                    right_client_id: plan.right_client_id.clone(),
                })),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT id, name, enabled, revision, left_client_id, right_client_id
                    FROM tunnel_plans
                    WHERE id = $1 AND deleted_at IS NULL
                    "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?;
                row.map(|row| {
                    Ok(TunnelPlanIdentity {
                        name: row.try_get("name")?,
                        enabled: row.try_get("enabled")?,
                        revision: row.try_get("revision")?,
                        left_client_id: row.try_get("left_client_id")?,
                        right_client_id: row.try_get("right_client_id")?,
                    })
                })
                .transpose()
            }
        }
    }

    pub(crate) async fn get_tunnel_plan_record(&self, id: Uuid) -> Result<Option<TunnelPlanView>> {
        Ok(match self {
            Self::Memory(memory) => memory
                .tunnel_plans
                .read()
                .await
                .iter()
                .find(|plan| plan.id == id && plan.deleted_at.is_none())
                .cloned(),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id, name, kind, enabled, revision, left_client_id, right_client_id,
                        input, plan, builtin_credentials, recommended_ospf_cost,
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
                    WHERE id = $1 AND deleted_at IS NULL
                    "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?;
                row.as_ref().map(tunnel_plan_from_row).transpose()?
            }
        })
    }

    pub(crate) async fn validate_tunnel_plan_resource_conflicts(
        &self,
        plan: &TunnelPlan,
        excluded_plan_id: Option<Uuid>,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let plans = memory.tunnel_plans.read().await;
                validate_memory_tunnel_plan_resource_conflicts(plan, &plans, excluded_plan_id)
            }
            Self::Postgres(pool) => {
                let rows = fetch_postgres_tunnel_plan_resource_rows(pool, excluded_plan_id).await?;
                validate_postgres_tunnel_plan_resource_rows(plan, rows)
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
        let ospf_endpoint_status = if plan.ospf.is_some() && enabled {
            "unverified"
        } else {
            "disabled"
        };
        let plan_id = Uuid::new_v4();
        let builtin_credentials = generate_tunnel_builtin_credentials(plan_id, plan, 1)?;
        let view = TunnelPlanView {
            id: plan_id,
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
            builtin_credentials,
            created_at: unix_now().to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            deleted_at: None,
            deleted_by: None,
            deleted_reason: None,
        };

        let persisted = match self {
            Self::Memory(memory) => {
                let _lifecycle = memory.agent_key_lifecycle.lock().await;
                require_visible_memory_clients(
                    memory,
                    &[view.left_client_id.clone(), view.right_client_id.clone()],
                    "tunnel_plan_endpoint_agent_not_found",
                )
                .await?;
                let mut plans = memory.tunnel_plans.write().await;
                if plans
                    .iter()
                    .any(|existing| existing.name == view.name && existing.deleted_at.is_none())
                {
                    anyhow::bail!("tunnel_plan_name_conflict");
                }
                validate_memory_tunnel_plan_resource_conflicts(plan, &plans, None)?;
                {
                    let definitions = memory.network_adapter_definitions.read().await;
                    validate_memory_tunnel_plan_adapter_references(&definitions, plan)?;
                }
                plans.push(view.clone());
                drop(plans);
                memory
                    .operational_alert_tunnel_plan_boundaries
                    .write()
                    .await
                    .insert(view.id, view.updated_at.clone());
                view.clone()
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_tunnel_plan_write(&mut tx).await?;
                validate_postgres_tunnel_plan_resource_conflicts(&mut tx, plan, None).await?;
                validate_postgres_tunnel_plan_adapter_references(&mut tx, plan).await?;
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
                        builtin_credentials, recommended_ospf_cost,
                        ospf_status, left_ospf_status, right_ospf_status
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                            $12, $12, $12)
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
                .bind(view.builtin_credentials.as_ref().map(SqlJson))
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
                reconcile_postgres_tunnel_alerts_for_clients_in_tx(
                    &mut tx,
                    &[
                        persisted.left_client_id.clone(),
                        persisted.right_client_id.clone(),
                    ],
                )
                .await?;
                tx.commit().await?;
                persisted
            }
        };

        if let Self::Memory(memory) = self {
            self.reconcile_memory_tunnel_alerts_for_clients(&[
                persisted.left_client_id.clone(),
                persisted.right_client_id.clone(),
            ])
            .await?;
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
        let previous = self
            .get_tunnel_plan_record(plan_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
        let mut affected_client_ids = HashSet::from([
            previous.left_client_id.clone(),
            previous.right_client_id.clone(),
            plan.left_client_id.clone(),
            plan.right_client_id.clone(),
        ])
        .into_iter()
        .collect::<Vec<_>>();
        affected_client_ids.sort();
        let ospf_endpoint_status = if plan.ospf.is_some() && enabled {
            "unverified"
        } else {
            "disabled"
        };
        let updated = match self {
            Self::Memory(memory) => {
                let _lifecycle = memory.agent_key_lifecycle.lock().await;
                require_visible_memory_clients(
                    memory,
                    &[plan.left_client_id.clone(), plan.right_client_id.clone()],
                    "tunnel_plan_endpoint_agent_not_found",
                )
                .await?;
                let mut plans = memory.tunnel_plans.write().await;
                let existing_index = plans
                    .iter()
                    .position(|existing| existing.id == plan_id && existing.deleted_at.is_none())
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                let existing = &plans[existing_index];
                if existing.revision != expected_revision {
                    anyhow::bail!("tunnel_plan_snapshot_stale");
                }
                if existing.name != plan.name {
                    anyhow::bail!("tunnel_plan_name_is_immutable");
                }
                let builtin_credentials = reconcile_tunnel_builtin_credentials(
                    plan_id,
                    Some(&existing.plan),
                    existing.builtin_credentials.as_ref(),
                    plan,
                )?;
                let previous_runtime_identity = tunnel_runtime_evidence_identity_hash(
                    plan_id,
                    &existing.plan,
                    existing
                        .builtin_credentials
                        .as_ref()
                        .map(TunnelBuiltinCredentials::generation),
                );
                let next_runtime_identity = tunnel_runtime_evidence_identity_hash(
                    plan_id,
                    plan,
                    builtin_credentials
                        .as_ref()
                        .map(TunnelBuiltinCredentials::generation),
                );
                let runtime_changed = existing.enabled != enabled
                    || previous_runtime_identity != next_runtime_identity;
                validate_memory_tunnel_plan_resource_conflicts(plan, &plans, Some(plan_id))?;
                {
                    let definitions = memory.network_adapter_definitions.read().await;
                    validate_memory_tunnel_plan_adapter_references(&definitions, plan)?;
                }
                let existing = &mut plans[existing_index];
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
                    builtin_credentials,
                    created_at: existing.created_at.clone(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                    deleted_at: None,
                    deleted_by: None,
                    deleted_reason: None,
                };
                *existing = updated.clone();
                drop(plans);
                if runtime_changed {
                    memory
                        .operational_alert_tunnel_plan_boundaries
                        .write()
                        .await
                        .insert(updated.id, updated.updated_at.clone());
                }
                updated
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_tunnel_plan_write(&mut tx).await?;
                lock_visible_postgres_tunnel_endpoints(
                    &mut tx,
                    &plan.left_client_id,
                    &plan.right_client_id,
                )
                .await?;
                let existing_row = sqlx::query(
                    r#"
                    SELECT name, revision, plan, builtin_credentials
                    FROM tunnel_plans
                    WHERE id = $1 AND deleted_at IS NULL
                    FOR UPDATE
                    "#,
                )
                .bind(plan_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                let persisted_name: String = existing_row.try_get("name")?;
                let persisted_revision: i64 = existing_row.try_get("revision")?;
                if persisted_revision != expected_revision {
                    anyhow::bail!("tunnel_plan_snapshot_stale");
                }
                if persisted_name != plan.name {
                    anyhow::bail!("tunnel_plan_name_is_immutable");
                }
                let previous_plan: SqlJson<serde_json::Value> = existing_row.try_get("plan")?;
                let previous_plan = serde_json::from_value::<TunnelPlan>(previous_plan.0);
                let previous_credentials = existing_row
                    .try_get::<Option<SqlJson<serde_json::Value>>, _>("builtin_credentials")?
                    .map(|value| serde_json::from_value::<TunnelBuiltinCredentials>(value.0))
                    .transpose();
                let builtin_credentials = match (&previous_plan, &previous_credentials) {
                    (Ok(previous_plan), Ok(previous_credentials)) => {
                        reconcile_tunnel_builtin_credentials(
                            plan_id,
                            Some(previous_plan),
                            previous_credentials.as_ref(),
                            plan,
                        )?
                    }
                    _ => {
                        let previous_plan_error =
                            previous_plan.as_ref().err().map(ToString::to_string);
                        let previous_credentials_error =
                            previous_credentials.as_ref().err().map(ToString::to_string);
                        warn!(
                            event = "tunnel_plan_configuration_repair",
                            %plan_id,
                            previous_plan_error = previous_plan_error.as_deref().unwrap_or("none"),
                            previous_credentials_error = previous_credentials_error
                                .as_deref()
                                .unwrap_or("none"),
                            "regenerating credentials while applying reviewed tunnel replacement"
                        );
                        let valid_previous_credentials =
                            previous_credentials.as_ref().ok().and_then(Option::as_ref);
                        let generation = match (valid_previous_credentials, plan.kind) {
                            (
                                Some(previous @ TunnelBuiltinCredentials::Wireguard { .. }),
                                TunnelKind::Wireguard,
                            )
                            | (
                                Some(previous @ TunnelBuiltinCredentials::Openvpn { .. }),
                                TunnelKind::Openvpn,
                            ) => next_credential_generation(previous)?,
                            _ => 1,
                        };
                        generate_tunnel_builtin_credentials(plan_id, plan, generation)?
                    }
                };
                validate_postgres_tunnel_plan_resource_conflicts(&mut tx, plan, Some(plan_id))
                    .await?;
                validate_postgres_tunnel_plan_adapter_references(&mut tx, plan).await?;
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
                        builtin_credentials = $8,
                        recommended_ospf_cost = $9,
                        ospf_status = $10,
                        left_ospf_status = $10,
                        right_ospf_status = $10,
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
                        updated_at = clock_timestamp()
                    WHERE id = $11
                      AND deleted_at IS NULL
                      AND name = $12
                      AND revision = $13
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
                .bind(builtin_credentials.as_ref().map(SqlJson))
                .bind(plan.recommended_ospf_cost.map(i32::from))
                .bind(ospf_endpoint_status)
                .bind(plan_id)
                .bind(&plan.name)
                .bind(expected_revision)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| anyhow::anyhow!("tunnel_plan_snapshot_stale"))?;
                let topology_identity_hash =
                    enabled.then(|| tunnel_topology_identity_hash(plan_id, plan));
                deactivate_postgres_automatic_observation_series_for_plan(
                    &mut tx,
                    plan_id,
                    topology_identity_hash.as_deref(),
                )
                .await?;
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
                    builtin_credentials,
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
                reconcile_postgres_tunnel_alerts_for_clients_in_tx(&mut tx, &affected_client_ids)
                    .await?;
                tx.commit().await?;
                updated
            }
        };

        if let Self::Memory(memory) = self {
            self.reconcile_memory_tunnel_alerts_for_clients(&affected_client_ids)
                .await?;
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
            .get_tunnel_plan_record(plan_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
        let ospf_status = if enabled && existing.plan.ospf.is_some() {
            "unverified"
        } else {
            "disabled"
        };
        match self {
            Self::Memory(memory) => {
                let _desired_state_guard = memory.agent_key_lifecycle.lock().await;
                let now = chrono::Utc::now().to_rfc3339();
                let runtime_changed;
                let updated = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let plan = plans
                        .iter_mut()
                        .find(|plan| plan.id == plan_id && plan.deleted_at.is_none())
                        .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                    if plan.revision != expected_revision {
                        anyhow::bail!("tunnel_plan_snapshot_stale");
                    }
                    runtime_changed = plan.enabled != enabled;
                    plan.enabled = enabled;
                    plan.revision += 1;
                    reset_ospf_runtime_state(plan, ospf_status);
                    reset_connection_assessment(plan);
                    plan.updated_at = now.clone();
                    plan.clone()
                };
                if runtime_changed {
                    memory
                        .operational_alert_tunnel_plan_boundaries
                        .write()
                        .await
                        .insert(updated.id, now.clone());
                }
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
                self.reconcile_memory_tunnel_alerts_for_clients(&[
                    updated.left_client_id.clone(),
                    updated.right_client_id.clone(),
                ])
                .await?;
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
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
                        updated_at = clock_timestamp()
                    WHERE id = $1 AND deleted_at IS NULL AND revision = $5
                    RETURNING
                        id, name, kind, enabled, revision, left_client_id, right_client_id,
                        input, plan, builtin_credentials, recommended_ospf_cost,
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
                    "#,
                )
                .bind(plan_id)
                .bind(enabled)
                .bind(persisted_actor_id(operator))
                .bind(ospf_status)
                .bind(expected_revision)
                .fetch_optional(&mut *tx)
                .await?;
                let row = row.ok_or_else(|| anyhow::anyhow!("tunnel_plan_snapshot_stale"))?;
                let updated = tunnel_plan_from_row(&row)?;
                if !enabled {
                    deactivate_postgres_automatic_observation_series_for_plan(
                        &mut tx, plan_id, None,
                    )
                    .await?;
                }
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
                .execute(&mut *tx)
                .await?;
                reconcile_postgres_tunnel_alerts_for_clients_in_tx(
                    &mut tx,
                    &[
                        existing.left_client_id.clone(),
                        existing.right_client_id.clone(),
                    ],
                )
                .await?;
                tx.commit().await?;
                self.get_tunnel_plan(plan_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))
            }
        }
    }

    pub(crate) async fn rotate_tunnel_plan_credentials(
        &self,
        plan_id: Uuid,
        expected_revision: i64,
        operator: &AuthContext,
    ) -> Result<TunnelPlanView> {
        let existing = self
            .get_tunnel_plan_record(plan_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
        if existing.revision != expected_revision {
            anyhow::bail!("tunnel_plan_snapshot_stale");
        }
        if existing.plan.runtime_control.manager != RuntimeTunnelManager::AgentBuiltin
            || !matches!(existing.kind, TunnelKind::Wireguard | TunnelKind::Openvpn)
        {
            anyhow::bail!("tunnel_plan_builtin_credentials_not_supported");
        }
        let generation = existing
            .builtin_credentials
            .as_ref()
            .map(next_credential_generation)
            .transpose()?
            .context("tunnel_plan_builtin_credentials_required")?;
        let credentials = generate_tunnel_builtin_credentials(plan_id, &existing.plan, generation)?
            .ok_or_else(|| anyhow::anyhow!("tunnel_plan_builtin_credentials_not_supported"))?;

        match self {
            Self::Memory(memory) => {
                let _desired_state_guard = memory.agent_key_lifecycle.lock().await;
                let now = chrono::Utc::now().to_rfc3339();
                let rotated = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let plan = plans
                        .iter_mut()
                        .find(|plan| plan.id == plan_id && plan.deleted_at.is_none())
                        .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                    if plan.revision != expected_revision {
                        anyhow::bail!("tunnel_plan_snapshot_stale");
                    }
                    plan.revision += 1;
                    plan.builtin_credentials = Some(credentials);
                    plan.updated_at = now.clone();
                    plan.clone()
                };
                memory
                    .operational_alert_tunnel_plan_boundaries
                    .write()
                    .await
                    .insert(rotated.id, now.clone());
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: persisted_actor_id(operator),
                    action: "network.tunnel_plan_credentials_rotated".to_string(),
                    target: format!("tunnel_plan:{plan_id}"),
                    command_hash: None,
                    metadata: tunnel_plan_metadata(&rotated, operator),
                    created_at: now,
                });
                self.reconcile_memory_tunnel_alerts_for_clients(&[
                    rotated.left_client_id.clone(),
                    rotated.right_client_id.clone(),
                ])
                .await?;
                Ok(rotated)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET actor_id = $2,
                        builtin_credentials = $3,
                        revision = revision + 1,
                        updated_at = clock_timestamp()
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND revision = $4
                    RETURNING revision, updated_at::text AS updated_at
                    "#,
                )
                .bind(plan_id)
                .bind(persisted_actor_id(operator))
                .bind(SqlJson(&credentials))
                .bind(expected_revision)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| anyhow::anyhow!("tunnel_plan_snapshot_stale"))?;
                let rotated = TunnelPlanView {
                    revision: row.try_get("revision")?,
                    builtin_credentials: Some(credentials),
                    updated_at: row.try_get("updated_at")?,
                    ..existing
                };
                insert_tunnel_audit(
                    &mut tx,
                    operator,
                    "network.tunnel_plan_credentials_rotated",
                    &rotated,
                    tunnel_plan_metadata(&rotated, operator),
                )
                .await?;
                reconcile_postgres_tunnel_alerts_for_clients_in_tx(
                    &mut tx,
                    &[
                        rotated.left_client_id.clone(),
                        rotated.right_client_id.clone(),
                    ],
                )
                .await?;
                tx.commit().await?;
                Ok(rotated)
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
                .bind(persisted_actor_id(operator))
                .bind(format!("tunnel_plan:{plan_id}"))
                .bind(network_audit_metadata(
                    serde_json::json!({
                        "assessment": assessment,
                        "note": note,
                    }),
                    operator,
                    "succeeded",
                ))
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
        let was_enabled = existing.enabled;
        match self {
            Self::Memory(memory) => {
                let _desired_state_guard = memory.agent_key_lifecycle.lock().await;
                let now = chrono::Utc::now().to_rfc3339();
                let deleted = {
                    let mut plans = memory.tunnel_plans.write().await;
                    let plan = plans
                        .iter_mut()
                        .find(|plan| plan.id == plan_id && plan.deleted_at.is_none())
                        .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                    if plan.revision != expected_revision {
                        anyhow::bail!("tunnel_plan_snapshot_stale");
                    }
                    plan.revision += 1;
                    plan.enabled = false;
                    plan.builtin_credentials = None;
                    plan.left_runtime_config = retired_tunnel_runtime_config(
                        plan.left_runtime_config.clone(),
                        was_enabled,
                    );
                    plan.right_runtime_config = retired_tunnel_runtime_config(
                        plan.right_runtime_config.clone(),
                        was_enabled,
                    );
                    plan.deleted_at = Some(now.clone());
                    plan.deleted_by = persisted_actor_id(operator);
                    plan.deleted_reason = Some("operator_retired".to_string());
                    plan.updated_at = now.clone();
                    plan.clone()
                };
                memory
                    .operational_alert_tunnel_plan_boundaries
                    .write()
                    .await
                    .insert(deleted.id, now.clone());
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: persisted_actor_id(operator),
                    action: "network.tunnel_plan_deleted".to_string(),
                    target: format!("tunnel_plan:{plan_id}"),
                    command_hash: None,
                    metadata: tunnel_plan_delete_metadata(&deleted, was_enabled, operator),
                    created_at: now,
                });
                self.reconcile_memory_tunnel_alerts_for_clients(&[
                    deleted.left_client_id.clone(),
                    deleted.right_client_id.clone(),
                ])
                .await?;
                Ok(deleted)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET actor_id = $2,
                        revision = revision + 1,
                        enabled = FALSE,
                        builtin_credentials = NULL,
                        deleted_at = now(),
                        deleted_by = $2,
                        deleted_reason = 'operator_retired',
                        updated_at = clock_timestamp()
                    WHERE id = $1
                      AND deleted_at IS NULL
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
                    enabled: false,
                    builtin_credentials: None,
                    left_runtime_config: retired_tunnel_runtime_config(
                        existing.left_runtime_config.clone(),
                        was_enabled,
                    ),
                    right_runtime_config: retired_tunnel_runtime_config(
                        existing.right_runtime_config.clone(),
                        was_enabled,
                    ),
                    revision: row.try_get("revision")?,
                    updated_at: row.try_get("updated_at")?,
                    deleted_at: row.try_get("deleted_at")?,
                    deleted_by: persisted_actor_id(operator),
                    deleted_reason: Some("operator_retired".to_string()),
                    ..existing
                };
                deactivate_postgres_automatic_observation_series_for_plan(&mut tx, plan_id, None)
                    .await?;
                insert_tunnel_audit(
                    &mut tx,
                    operator,
                    "network.tunnel_plan_deleted",
                    &deleted,
                    tunnel_plan_delete_metadata(&deleted, was_enabled, operator),
                )
                .await?;
                reconcile_postgres_tunnel_alerts_for_clients_in_tx(
                    &mut tx,
                    &[
                        deleted.left_client_id.clone(),
                        deleted.right_client_id.clone(),
                    ],
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
            .get_tunnel_plan_record(plan_id)
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
                    plan.updated_at = chrono::Utc::now().to_rfc3339();
                    plan.clone()
                };
                memory
                    .pending_ospf_plan_reconciliations
                    .write()
                    .await
                    .remove(&plan_id);
                memory.audits.write().await.push(ospf_jobs_audit(
                    &updated,
                    left_job_id,
                    right_job_id,
                    operator,
                ));
                self.reconcile_memory_tunnel_alerts_for_clients(&[
                    updated.left_client_id.clone(),
                    updated.right_client_id.clone(),
                ])
                .await?;
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE tunnel_plans
                    SET actor_id = $2,
                        desired_ospf_cost = $3,
                        ospf_status = 'pending',
                        left_ospf_status = 'pending',
                        right_ospf_status = 'pending',
                        pending_ospf_reconciled_at = NULL,
                        left_ospf_job_id = $4,
                        right_ospf_job_id = $5,
                        updated_at = clock_timestamp()
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND enabled = TRUE
                      AND revision = $8
                      AND left_current_ospf_cost IS NOT DISTINCT FROM $6
                      AND right_current_ospf_cost IS NOT DISTINCT FROM $7
                      AND left_ospf_status <> 'pending'
                      AND right_ospf_status <> 'pending'
                    RETURNING
                        id, name, kind, enabled, revision, left_client_id, right_client_id,
                        input, plan, builtin_credentials, recommended_ospf_cost,
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
                .fetch_optional(&mut *tx)
                .await?;
                let row = row.ok_or_else(|| anyhow::anyhow!("tunnel_plan_ospf_snapshot_stale"))?;
                let updated = tunnel_plan_from_row(&row)?;
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
                .execute(&mut *tx)
                .await?;
                reconcile_postgres_tunnel_alerts_for_clients_in_tx(
                    &mut tx,
                    &[
                        updated.left_client_id.clone(),
                        updated.right_client_id.clone(),
                    ],
                )
                .await?;
                tx.commit().await?;
                self.get_tunnel_plan(plan_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))
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
                let updated = {
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
                    plan.updated_at = chrono::Utc::now().to_rfc3339();
                    plan.clone()
                };
                self.reconcile_memory_tunnel_alerts_for_clients(&[
                    updated.left_client_id.clone(),
                    updated.right_client_id.clone(),
                ])
                .await?;
                Ok(Some(updated))
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
                    "UPDATE tunnel_plans SET {status_column} = $3, {cost_column} = $4, updated_at = clock_timestamp() \
                     WHERE id = $1 AND deleted_at IS NULL AND {job_column} = $2 \
                       AND {status_column} = 'pending' \
                     RETURNING enabled, plan, desired_ospf_cost, \
                        left_ospf_status, right_ospf_status, \
                        left_current_ospf_cost, right_current_ospf_cost"
                );
                let mut tx = pool.begin().await?;
                let row = sqlx::query(&query)
                    .bind(plan_id)
                    .bind(job_id)
                    .bind(if succeeded { "verified" } else { "failed" })
                    .bind(current_cost.map(i32::from))
                    .fetch_optional(&mut *tx)
                    .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let plan: SqlJson<TunnelPlan> = row.try_get("plan")?;
                let ospf_status = aggregate_ospf_status_fields(
                    row.try_get("enabled")?,
                    plan.0.ospf.is_some(),
                    row.try_get::<String, _>("left_ospf_status")?.as_str(),
                    row.try_get::<String, _>("right_ospf_status")?.as_str(),
                    row.try_get("desired_ospf_cost")?,
                    row.try_get("left_current_ospf_cost")?,
                    row.try_get("right_current_ospf_cost")?,
                );
                sqlx::query(
                    "UPDATE tunnel_plans SET ospf_status = $2, updated_at = clock_timestamp() WHERE id = $1",
                )
                .bind(plan_id)
                .bind(ospf_status)
                .execute(&mut *tx)
                .await?;
                let endpoints = sqlx::query(
                    "SELECT left_client_id, right_client_id FROM tunnel_plans WHERE id = $1",
                )
                .bind(plan_id)
                .fetch_one(&mut *tx)
                .await?;
                reconcile_postgres_tunnel_alerts_for_clients_in_tx(
                    &mut tx,
                    &[
                        endpoints.try_get("left_client_id")?,
                        endpoints.try_get("right_client_id")?,
                    ],
                )
                .await?;
                tx.commit().await?;
                self.get_tunnel_plan(plan_id).await
            }
        }
    }
}

fn tunnel_plan_from_row(row: &sqlx::postgres::PgRow) -> Result<TunnelPlanView> {
    let input: SqlJson<serde_json::Value> = row.try_get("input")?;
    let plan: SqlJson<serde_json::Value> = row.try_get("plan")?;
    let builtin_credentials = row
        .try_get::<Option<SqlJson<serde_json::Value>>, _>("builtin_credentials")?
        .map(|value| {
            serde_json::from_value(value.0).context("invalid persisted tunnel builtin credentials")
        })
        .transpose()?;
    let input = serde_json::from_value::<TunnelPlanInput>(input.0)
        .map_err(|error| anyhow::anyhow!("invalid persisted tunnel input: {error}"))?;
    let plan = serde_json::from_value::<TunnelPlan>(plan.0)
        .map_err(|error| anyhow::anyhow!("invalid persisted tunnel plan: {error}"))?;
    Ok(TunnelPlanView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        kind: parse_tunnel_kind(row.try_get::<String, _>("kind")?.as_str())?,
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
        input,
        plan,
        builtin_credentials,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted_at: row.try_get("deleted_at")?,
        deleted_by: row.try_get("deleted_by")?,
        deleted_reason: row.try_get("deleted_reason")?,
    })
}

fn tunnel_plan_corrupt_from_row(
    row: &sqlx::postgres::PgRow,
    error: &anyhow::Error,
) -> Result<TunnelPlanCorruptView> {
    Ok(TunnelPlanCorruptView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        kind: row.try_get("kind")?,
        enabled: row.try_get("enabled")?,
        revision: row.try_get("revision")?,
        left_client_id: row.try_get("left_client_id")?,
        right_client_id: row.try_get("right_client_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted_at: row.try_get("deleted_at")?,
        configuration_error: format!("Persisted tunnel configuration is invalid: {error}"),
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
        job_id: None,
        error: None,
        updated_at: None,
    }
}

fn retired_tunnel_runtime_config(
    mut state: TunnelPlanEndpointRuntimeConfigView,
    was_enabled: bool,
) -> TunnelPlanEndpointRuntimeConfigView {
    state.desired = "absent".to_string();
    if was_enabled {
        state.status = "removal_required".to_string();
        state.job_id = None;
        state.error = None;
    }
    state
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
                job_id: state.pending_job_id,
                error: state.pending_error.clone(),
                updated_at: state.pending_updated_at.clone(),
            };
        }
        return TunnelPlanEndpointRuntimeConfigView {
            client_id: client_id.to_string(),
            desired: desired.to_string(),
            status: "stale_pending".to_string(),
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

const TUNNEL_PLAN_RESOURCE_ROWS_QUERY: &str = r#"
    SELECT id, left_client_id, right_client_id, plan
    FROM tunnel_plans
    WHERE deleted_at IS NULL
      AND ($1::uuid IS NULL OR id <> $1)
    ORDER BY id
"#;

fn tunnel_plan_addresses(plan: &TunnelPlan) -> Result<HashSet<IpAddr>> {
    [plan.ipv4_tunnel.as_ref(), plan.ipv6_tunnel.as_ref()]
        .into_iter()
        .flatten()
        .flat_map(|pair| [&pair.left, &pair.right])
        .map(|address| {
            address
                .parse()
                .map_err(|_| anyhow::anyhow!("tunnel_plan_address_invalid"))
        })
        .collect()
}

fn validate_tunnel_plan_resource_pair(
    requested: &TunnelPlan,
    requested_addresses: &HashSet<IpAddr>,
    existing: &TunnelPlan,
    existing_left_client_id: &str,
    existing_right_client_id: &str,
) -> Result<()> {
    let shares_endpoint = [
        requested.left_client_id.as_str(),
        requested.right_client_id.as_str(),
    ]
    .into_iter()
    .any(|client_id| client_id == existing_left_client_id || client_id == existing_right_client_id);
    anyhow::ensure!(
        !shares_endpoint || requested.interface_name != existing.interface_name,
        "tunnel_plan_interface_conflict"
    );
    anyhow::ensure!(
        requested_addresses.is_disjoint(&tunnel_plan_addresses(existing)?),
        "tunnel_plan_address_conflict"
    );
    let existing_ports = tunnel_plan_listener_resources(existing)
        .into_iter()
        .collect::<HashSet<_>>();
    anyhow::ensure!(
        tunnel_plan_listener_resources(requested)
            .into_iter()
            .all(|resource| !existing_ports.contains(&resource)),
        "tunnel_plan_listener_port_conflict"
    );
    Ok(())
}

fn tunnel_plan_listener_resources(plan: &TunnelPlan) -> Vec<(String, &'static str, u16)> {
    if plan.runtime_control.manager != RuntimeTunnelManager::AgentBuiltin {
        return Vec::new();
    }
    match plan.kind {
        TunnelKind::Fou => vec![
            (
                plan.left_client_id.clone(),
                "udp",
                plan.runtime_control.fou.port,
            ),
            (
                plan.right_client_id.clone(),
                "udp",
                plan.runtime_control.fou.port,
            ),
        ],
        TunnelKind::Wireguard => vec![
            (
                plan.left_client_id.clone(),
                "udp",
                plan.runtime_control.wireguard.left_listen_port,
            ),
            (
                plan.right_client_id.clone(),
                "udp",
                plan.runtime_control.wireguard.right_listen_port,
            ),
        ],
        TunnelKind::Openvpn => {
            let client_id = match plan.runtime_control.openvpn.listener_side {
                TunnelEndpointSide::Left => &plan.left_client_id,
                TunnelEndpointSide::Right => &plan.right_client_id,
            };
            let transport = match plan.runtime_control.openvpn.transport {
                vpsman_common::RuntimeTunnelOpenvpnTransport::Udp => "udp",
                vpsman_common::RuntimeTunnelOpenvpnTransport::Tcp => "tcp",
            };
            vec![(
                client_id.clone(),
                transport,
                plan.runtime_control.openvpn.port,
            )]
        }
        _ => Vec::new(),
    }
}

fn validate_memory_tunnel_plan_resource_conflicts(
    requested: &TunnelPlan,
    existing_plans: &[TunnelPlanView],
    excluded_plan_id: Option<Uuid>,
) -> Result<()> {
    let requested_addresses = tunnel_plan_addresses(requested)?;
    for existing in existing_plans
        .iter()
        .filter(|existing| existing.deleted_at.is_none())
        .filter(|existing| Some(existing.id) != excluded_plan_id)
    {
        validate_tunnel_plan_resource_pair(
            requested,
            &requested_addresses,
            &existing.plan,
            &existing.left_client_id,
            &existing.right_client_id,
        )?;
    }
    Ok(())
}

async fn fetch_postgres_tunnel_plan_resource_rows(
    pool: &sqlx::PgPool,
    excluded_plan_id: Option<Uuid>,
) -> Result<Vec<sqlx::postgres::PgRow>> {
    Ok(sqlx::query(TUNNEL_PLAN_RESOURCE_ROWS_QUERY)
        .bind(excluded_plan_id)
        .fetch_all(pool)
        .await?)
}

fn validate_postgres_tunnel_plan_resource_rows(
    requested: &TunnelPlan,
    rows: Vec<sqlx::postgres::PgRow>,
) -> Result<()> {
    let requested_addresses = tunnel_plan_addresses(requested)?;
    for row in rows {
        let existing: SqlJson<serde_json::Value> = row.try_get("plan")?;
        let existing = serde_json::from_value::<TunnelPlan>(existing.0)
            .map_err(|error| anyhow::anyhow!("invalid persisted tunnel plan: {error}"))?;
        validate_tunnel_plan_resource_pair(
            requested,
            &requested_addresses,
            &existing,
            &row.try_get::<String, _>("left_client_id")?,
            &row.try_get::<String, _>("right_client_id")?,
        )?;
    }
    Ok(())
}

async fn validate_postgres_tunnel_plan_resource_conflicts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    requested: &TunnelPlan,
    excluded_plan_id: Option<Uuid>,
) -> Result<()> {
    let rows = sqlx::query(TUNNEL_PLAN_RESOURCE_ROWS_QUERY)
        .bind(excluded_plan_id)
        .fetch_all(&mut **tx)
        .await?;
    validate_postgres_tunnel_plan_resource_rows(requested, rows)
}

async fn lock_postgres_tunnel_plan_write(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    // Keep the global lifecycle -> desired-source table order used by runtime
    // config commits. Endpoint validation below takes this same advisory lock,
    // so taking it first prevents a guard/writer table-lock cycle.
    lock_postgres_agent_identity_lifecycle(tx).await?;
    // Serialize only plan create/update conflict scans. Other tunnel status writes keep
    // normal row-level concurrency.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind("vpsman.tunnel_plan_resource_conflicts")
        .execute(&mut **tx)
        .await?;
    // Adapter definition mutation takes SHARE before checking references. ROW EXCLUSIVE
    // preserves that table -> definition lock order while remaining writer-compatible.
    sqlx::query("LOCK TABLE tunnel_plans IN ROW EXCLUSIVE MODE")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(Clone, Copy)]
struct TunnelPlanAdapterReference {
    id: Uuid,
    adapter_kind: &'static str,
}

fn tunnel_plan_adapter_references(plan: &TunnelPlan) -> Result<Vec<TunnelPlanAdapterReference>> {
    let mut references = Vec::with_capacity(4);
    let mut add_reference = |raw_id: &str, adapter_kind: &'static str| -> Result<()> {
        let id = Uuid::parse_str(raw_id)
            .map_err(|_| anyhow::anyhow!("tunnel_plan_adapter_definition_id_invalid"))?;
        if !references
            .iter()
            .any(|reference: &TunnelPlanAdapterReference| {
                reference.id == id && reference.adapter_kind == adapter_kind
            })
        {
            references.push(TunnelPlanAdapterReference { id, adapter_kind });
        }
        Ok(())
    };

    if plan.runtime_control.manager == RuntimeTunnelManager::CustomAdapter {
        let left_id = plan
            .runtime_control
            .left_adapter_definition_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("tunnel_plan_adapter_definition_id_invalid"))?;
        let right_id = plan
            .runtime_control
            .right_adapter_definition_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("tunnel_plan_adapter_definition_id_invalid"))?;
        add_reference(left_id, "runtime_tunnel")?;
        add_reference(right_id, "runtime_tunnel")?;
    }
    if let Some(ospf) = &plan.ospf {
        if let Some(id) = ospf.left_adapter_definition_id.as_deref() {
            add_reference(id, "routing_cost")?;
        }
        if let Some(id) = ospf.right_adapter_definition_id.as_deref() {
            add_reference(id, "routing_cost")?;
        }
    }
    Ok(references)
}

fn validate_memory_tunnel_plan_adapter_references(
    definitions: &[NetworkAdapterDefinitionView],
    plan: &TunnelPlan,
) -> Result<()> {
    for reference in tunnel_plan_adapter_references(plan)? {
        anyhow::ensure!(
            definitions.iter().any(|definition| {
                definition.id == reference.id && definition.adapter_kind == reference.adapter_kind
            }),
            "tunnel_plan_adapter_definition_unavailable"
        );
    }
    Ok(())
}

async fn validate_postgres_tunnel_plan_adapter_references(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    plan: &TunnelPlan,
) -> Result<()> {
    for reference in tunnel_plan_adapter_references(plan)? {
        let adapter_kind = sqlx::query_scalar::<_, String>(
            r#"
            SELECT adapter_kind
            FROM network_adapter_definitions
            WHERE id = $1
            FOR SHARE
            "#,
        )
        .bind(reference.id)
        .fetch_optional(&mut **tx)
        .await?;
        anyhow::ensure!(
            adapter_kind.as_deref() == Some(reference.adapter_kind),
            "tunnel_plan_adapter_definition_unavailable"
        );
    }
    Ok(())
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
    require_visible_postgres_clients_in_tx(
        tx,
        &[left_client_id.to_string(), right_client_id.to_string()],
        "tunnel_plan_endpoint_agent_not_found",
    )
    .await?;
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
    aggregate_ospf_status_fields(
        plan.enabled,
        plan.plan.ospf.is_some(),
        &plan.left_ospf_status,
        &plan.right_ospf_status,
        plan.desired_ospf_cost,
        plan.left_current_ospf_cost,
        plan.right_current_ospf_cost,
    )
}

fn aggregate_ospf_status_fields(
    enabled: bool,
    ospf_enabled: bool,
    left: &str,
    right: &str,
    desired_ospf_cost: Option<i32>,
    left_current_ospf_cost: Option<i32>,
    right_current_ospf_cost: Option<i32>,
) -> &'static str {
    if !enabled || !ospf_enabled {
        return "disabled";
    }
    if left == "pending" || right == "pending" {
        return "pending";
    }
    if left == "verified" && right == "verified" {
        if let Some(desired) = desired_ospf_cost {
            if left_current_ospf_cost != Some(desired) || right_current_ospf_cost != Some(desired) {
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
    network_audit_metadata(
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
        }),
        operator,
        "succeeded",
    )
}

fn tunnel_plan_enabled_metadata(
    view: &TunnelPlanView,
    operator: &AuthContext,
) -> serde_json::Value {
    network_audit_metadata(
        serde_json::json!({
            "name": &view.name,
            "enabled": view.enabled,
            "left_client_id": &view.left_client_id,
            "right_client_id": &view.right_client_id,
            "ospf_status": &view.ospf_status,
        }),
        operator,
        "succeeded",
    )
}

fn tunnel_connection_assessment_metadata(
    view: &TunnelPlanView,
    operator: &AuthContext,
) -> serde_json::Value {
    network_audit_metadata(
        serde_json::json!({
            "name": &view.name,
            "revision": view.revision,
            "assessment": &view.connection_assessment,
            "note": &view.connection_assessment_note,
        }),
        operator,
        "succeeded",
    )
}

fn tunnel_plan_delete_metadata(
    view: &TunnelPlanView,
    was_enabled: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    network_audit_metadata(
        serde_json::json!({
            "name": &view.name,
            "revision": view.revision,
            "was_enabled": was_enabled,
            "left_client_id": &view.left_client_id,
            "right_client_id": &view.right_client_id,
            "interface_name": &view.plan.interface_name,
            "deleted_reason": &view.deleted_reason,
        }),
        operator,
        "succeeded",
    )
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
    network_audit_metadata(
        serde_json::json!({
            "plan_id": view.id,
            "plan_name": &view.name,
            "desired_ospf_cost": view.desired_ospf_cost,
            "left_current_ospf_cost": view.left_current_ospf_cost,
            "right_current_ospf_cost": view.right_current_ospf_cost,
            "left_job_id": left_job_id,
            "right_job_id": right_job_id,
        }),
        operator,
        "requested",
    )
}

pub(crate) fn network_audit_metadata(
    mut metadata: serde_json::Value,
    operator: &AuthContext,
    result: &str,
) -> serde_json::Value {
    let fields = metadata
        .as_object_mut()
        .expect("network audit metadata must be an object");
    fields.insert("result".to_string(), serde_json::json!(result));
    if let Some(operator_id) = persisted_actor_id(operator) {
        fields.insert(
            "origin_kind".to_string(),
            serde_json::json!("operator_request"),
        );
        fields.insert(
            "component".to_string(),
            serde_json::json!("network-controller"),
        );
        fields.insert("operator_id".to_string(), serde_json::json!(operator_id));
        fields.insert(
            "operator_username".to_string(),
            serde_json::json!(&operator.operator.username),
        );
        fields.insert(
            "operator_role".to_string(),
            serde_json::json!(&operator.operator.role),
        );
        fields.insert(
            "operator_session_id".to_string(),
            serde_json::json!(operator.audit_session_id()),
        );
    } else {
        fields.insert(
            "origin_kind".to_string(),
            serde_json::json!("control_plane"),
        );
        fields.insert(
            "component".to_string(),
            serde_json::json!(&operator.operator.username),
        );
    }
    metadata
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
    .bind(persisted_actor_id(operator))
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

fn parse_tunnel_kind(value: &str) -> Result<TunnelKind> {
    Ok(match value {
        "gre" => TunnelKind::Gre,
        "ipip" => TunnelKind::Ipip,
        "sit" => TunnelKind::Sit,
        "fou" => TunnelKind::Fou,
        "openvpn" => TunnelKind::Openvpn,
        "wireguard" => TunnelKind::Wireguard,
        "tun_tap" => TunnelKind::TunTap,
        "custom" => TunnelKind::Custom,
        _ => anyhow::bail!("invalid persisted tunnel kind: {value}"),
    })
}

fn runtime_manager_name(manager: RuntimeTunnelManager) -> &'static str {
    match manager {
        RuntimeTunnelManager::AgentBuiltin => "agent_builtin",
        RuntimeTunnelManager::ExternalObserved => "external_observed",
        RuntimeTunnelManager::CustomAdapter => "custom_adapter",
    }
}
