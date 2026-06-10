use anyhow::{Context, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    fleet_alerts::FleetAlertPolicy,
    model::AuditLogView,
    model::AuthContext,
    model_alert_policies::{CreateFleetAlertPolicyRequest, FleetAlertPolicyOverrideView},
    repository::Repository,
    unix_now,
};

const SCOPE_GLOBAL: &str = "global";
const SCOPE_PROVIDER: &str = "provider";
const SCOPE_TAG: &str = "tag";
const SCOPE_CLIENT: &str = "client";
const MAX_ALERT_POLICY_NAME_BYTES: usize = 128;
const MAX_ALERT_POLICY_SCOPE_BYTES: usize = 128;
const MAX_ALERT_POLICY_NOTES_BYTES: usize = 1024;

impl Repository {
    pub(crate) async fn list_fleet_alert_policies(
        &self,
        limit: i64,
        enabled: Option<bool>,
        scope_kind: Option<&str>,
        scope_value: Option<&str>,
    ) -> Result<Vec<FleetAlertPolicyOverrideView>> {
        let scope_kind = normalize_optional_scope_kind(scope_kind)?;
        let scope_value = scope_value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .fleet_alert_policies
                    .read()
                    .await
                    .iter()
                    .filter(|policy| enabled.is_none_or(|value| policy.enabled == value))
                    .filter(|policy| {
                        scope_kind
                            .as_deref()
                            .is_none_or(|value| policy.scope_kind == value)
                    })
                    .filter(|policy| {
                        scope_value
                            .as_deref()
                            .is_none_or(|value| policy.scope_value.as_deref() == Some(value))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                sort_alert_policies(&mut rows);
                rows.truncate(limit.clamp(1, 1000) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        memory_available_warning_ratio,
                        memory_available_critical_ratio,
                        disk_available_warning_ratio,
                        disk_available_critical_ratio,
                        cpu_load_warning,
                        cpu_load_critical,
                        priority,
                        enabled,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM fleet_alert_policies
                    WHERE ($2::boolean IS NULL OR enabled = $2)
                      AND ($3::text IS NULL OR scope_kind = $3)
                      AND ($4::text IS NULL OR scope_value = $4)
                    ORDER BY enabled DESC, priority DESC, scope_kind, name
                    LIMIT $1
                    "#,
                )
                .bind(limit.clamp(1, 1000))
                .bind(enabled)
                .bind(scope_kind.as_deref())
                .bind(scope_value.as_deref())
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(alert_policy_from_row).collect()
            }
        }
    }

    pub(crate) async fn upsert_fleet_alert_policy(
        &self,
        request: &CreateFleetAlertPolicyRequest,
        operator: &AuthContext,
    ) -> Result<FleetAlertPolicyOverrideView> {
        let candidate = alert_policy_from_request(request, operator)?;
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut policies = memory.fleet_alert_policies.write().await;
                anyhow::ensure!(
                    !policies.iter().any(|stored| {
                        stored.name == candidate.name && Some(stored.id) != request.id
                    }),
                    "fleet_alert_policy_name_conflict"
                );
                let policy = if let Some(stored) = policies
                    .iter_mut()
                    .find(|stored| request.id.is_some_and(|id| stored.id == id))
                {
                    stored.name = candidate.name.clone();
                    stored.scope_kind = candidate.scope_kind.clone();
                    stored.scope_value = candidate.scope_value.clone();
                    stored.memory_available_warning_ratio =
                        candidate.memory_available_warning_ratio;
                    stored.memory_available_critical_ratio =
                        candidate.memory_available_critical_ratio;
                    stored.disk_available_warning_ratio = candidate.disk_available_warning_ratio;
                    stored.disk_available_critical_ratio = candidate.disk_available_critical_ratio;
                    stored.cpu_load_warning = candidate.cpu_load_warning;
                    stored.cpu_load_critical = candidate.cpu_load_critical;
                    stored.priority = candidate.priority;
                    stored.enabled = candidate.enabled;
                    stored.notes = candidate.notes.clone();
                    stored.actor_id = candidate.actor_id;
                    stored.updated_at = now.clone();
                    stored.clone()
                } else {
                    policies.push(candidate.clone());
                    candidate
                };
                drop(policies);
                memory
                    .audits
                    .write()
                    .await
                    .push(alert_policy_audit(&policy, operator, now));
                Ok(policy)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO fleet_alert_policies (
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        memory_available_warning_ratio,
                        memory_available_critical_ratio,
                        disk_available_warning_ratio,
                        disk_available_critical_ratio,
                        cpu_load_warning,
                        cpu_load_critical,
                        priority,
                        enabled,
                        notes,
                        actor_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                    ON CONFLICT (id) DO UPDATE SET
                        name = EXCLUDED.name,
                        scope_kind = EXCLUDED.scope_kind,
                        scope_value = EXCLUDED.scope_value,
                        memory_available_warning_ratio = EXCLUDED.memory_available_warning_ratio,
                        memory_available_critical_ratio = EXCLUDED.memory_available_critical_ratio,
                        disk_available_warning_ratio = EXCLUDED.disk_available_warning_ratio,
                        disk_available_critical_ratio = EXCLUDED.disk_available_critical_ratio,
                        cpu_load_warning = EXCLUDED.cpu_load_warning,
                        cpu_load_critical = EXCLUDED.cpu_load_critical,
                        priority = EXCLUDED.priority,
                        enabled = EXCLUDED.enabled,
                        notes = EXCLUDED.notes,
                        actor_id = EXCLUDED.actor_id,
                        updated_at = now()
                    RETURNING
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        memory_available_warning_ratio,
                        memory_available_critical_ratio,
                        disk_available_warning_ratio,
                        disk_available_critical_ratio,
                        cpu_load_warning,
                        cpu_load_critical,
                        priority,
                        enabled,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(candidate.id)
                .bind(&candidate.name)
                .bind(&candidate.scope_kind)
                .bind(&candidate.scope_value)
                .bind(candidate.memory_available_warning_ratio)
                .bind(candidate.memory_available_critical_ratio)
                .bind(candidate.disk_available_warning_ratio)
                .bind(candidate.disk_available_critical_ratio)
                .bind(candidate.cpu_load_warning)
                .bind(candidate.cpu_load_critical)
                .bind(candidate.priority)
                .bind(candidate.enabled)
                .bind(&candidate.notes)
                .bind(operator.operator.id)
                .fetch_one(&mut *tx)
                .await?;
                let policy = alert_policy_from_row(row)?;
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
                .bind("fleet.alert_policy_upserted")
                .bind(format!("fleet_alert_policy:{}", policy.id))
                .bind(alert_policy_metadata(&policy, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(policy)
            }
        }
    }

    pub(crate) async fn delete_fleet_alert_policy(
        &self,
        policy_id: Uuid,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut policies = memory.fleet_alert_policies.write().await;
                let index = policies
                    .iter()
                    .position(|policy| policy.id == policy_id)
                    .ok_or_else(|| anyhow::anyhow!("fleet_alert_policy_not_found:{policy_id}"))?;
                let policy = policies.remove(index);
                drop(policies);
                let mut audit = alert_policy_audit(&policy, operator, unix_now().to_string());
                audit.action = "fleet.alert_policy_deleted".to_string();
                memory.audits.write().await.push(audit);
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    DELETE FROM fleet_alert_policies
                    WHERE id = $1
                    RETURNING
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        memory_available_warning_ratio,
                        memory_available_critical_ratio,
                        disk_available_warning_ratio,
                        disk_available_critical_ratio,
                        cpu_load_warning,
                        cpu_load_critical,
                        priority,
                        enabled,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(policy_id)
                .fetch_one(&mut *tx)
                .await?;
                let policy = alert_policy_from_row(row)?;
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
                .bind("fleet.alert_policy_deleted")
                .bind(format!("fleet_alert_policy:{}", policy.id))
                .bind(alert_policy_metadata(&policy, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }
}

fn alert_policy_from_request(
    request: &CreateFleetAlertPolicyRequest,
    operator: &AuthContext,
) -> Result<FleetAlertPolicyOverrideView> {
    anyhow::ensure!(
        request.confirmed,
        "fleet_alert_policy_confirmation_required"
    );
    validate_alert_policy_name(&request.name)?;
    let scope_kind = normalize_scope_kind(&request.scope_kind)?;
    let scope_value = normalize_scope_value(&scope_kind, request.scope_value.as_deref())?;
    validate_optional_notes(request.notes.as_deref())?;
    let policy = FleetAlertPolicyOverrideView {
        id: request.id.unwrap_or_else(Uuid::new_v4),
        name: request.name.trim().to_string(),
        scope_kind,
        scope_value,
        memory_available_warning_ratio: request.memory_available_warning_ratio,
        memory_available_critical_ratio: request.memory_available_critical_ratio,
        disk_available_warning_ratio: request.disk_available_warning_ratio,
        disk_available_critical_ratio: request.disk_available_critical_ratio,
        cpu_load_warning: request.cpu_load_warning,
        cpu_load_critical: request.cpu_load_critical,
        priority: request.priority.unwrap_or(0),
        enabled: request.enabled.unwrap_or(true),
        notes: request
            .notes
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        actor_id: Some(operator.operator.id),
        created_at: unix_now().to_string(),
        updated_at: unix_now().to_string(),
    };
    FleetAlertPolicy::validate_override(&policy)?;
    Ok(policy)
}

fn validate_alert_policy_name(name: &str) -> Result<()> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "fleet alert policy name is required");
    anyhow::ensure!(
        name.len() <= MAX_ALERT_POLICY_NAME_BYTES,
        "fleet alert policy name is too long"
    );
    anyhow::ensure!(
        name.bytes()
            .all(|byte| byte.is_ascii_alphanumeric()
                || matches!(byte, b' ' | b'.' | b'_' | b'-' | b':')),
        "fleet alert policy name contains unsupported characters"
    );
    Ok(())
}

fn validate_optional_notes(notes: Option<&str>) -> Result<()> {
    if let Some(notes) = notes {
        anyhow::ensure!(
            notes.len() <= MAX_ALERT_POLICY_NOTES_BYTES,
            "fleet alert policy notes are too long"
        );
    }
    Ok(())
}

fn normalize_optional_scope_kind(scope_kind: Option<&str>) -> Result<Option<String>> {
    scope_kind
        .map(normalize_scope_kind)
        .transpose()
        .context("invalid fleet alert policy scope kind")
}

fn normalize_scope_kind(scope_kind: &str) -> Result<String> {
    let scope_kind = scope_kind.trim().to_ascii_lowercase();
    anyhow::ensure!(
        matches!(
            scope_kind.as_str(),
            SCOPE_GLOBAL | SCOPE_PROVIDER | SCOPE_TAG | SCOPE_CLIENT
        ),
        "fleet alert policy scope kind is invalid"
    );
    Ok(scope_kind)
}

fn normalize_scope_value(scope_kind: &str, scope_value: Option<&str>) -> Result<Option<String>> {
    let scope_value = scope_value.map(str::trim).filter(|value| !value.is_empty());
    if scope_kind == SCOPE_GLOBAL {
        anyhow::ensure!(
            scope_value.is_none(),
            "global fleet alert policies must not include a scope value"
        );
        return Ok(None);
    }
    let scope_value = scope_value.context("scoped fleet alert policies require a scope value")?;
    anyhow::ensure!(
        scope_value.len() <= MAX_ALERT_POLICY_SCOPE_BYTES,
        "fleet alert policy scope value is too long"
    );
    Ok(Some(scope_value.to_string()))
}

fn sort_alert_policies(policies: &mut [FleetAlertPolicyOverrideView]) {
    policies.sort_by(|left, right| {
        right
            .enabled
            .cmp(&left.enabled)
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| left.scope_kind.cmp(&right.scope_kind))
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn alert_policy_from_row(row: sqlx::postgres::PgRow) -> Result<FleetAlertPolicyOverrideView> {
    Ok(FleetAlertPolicyOverrideView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        scope_kind: row.try_get("scope_kind")?,
        scope_value: row.try_get("scope_value")?,
        memory_available_warning_ratio: row.try_get("memory_available_warning_ratio")?,
        memory_available_critical_ratio: row.try_get("memory_available_critical_ratio")?,
        disk_available_warning_ratio: row.try_get("disk_available_warning_ratio")?,
        disk_available_critical_ratio: row.try_get("disk_available_critical_ratio")?,
        cpu_load_warning: row.try_get("cpu_load_warning")?,
        cpu_load_critical: row.try_get("cpu_load_critical")?,
        priority: row.try_get("priority")?,
        enabled: row.try_get("enabled")?,
        notes: row.try_get("notes")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn alert_policy_audit(
    policy: &FleetAlertPolicyOverrideView,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "fleet.alert_policy_upserted".to_string(),
        target: format!("fleet_alert_policy:{}", policy.id),
        command_hash: None,
        metadata: alert_policy_metadata(policy, operator),
        created_at,
    }
}

fn alert_policy_metadata(
    policy: &FleetAlertPolicyOverrideView,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "operator": operator.operator.username,
        "policy": policy,
    })
}
