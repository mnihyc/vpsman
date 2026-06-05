use anyhow::Result;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;

use crate::job_request::job_command_type_label;
use crate::model::*;
use crate::repository::Repository;
use crate::unix_now;

impl Repository {
    pub(crate) async fn list_schedules(&self) -> Result<Vec<ScheduleView>> {
        match self {
            Self::Memory(memory) => {
                let mut schedules = memory.schedules.read().await.clone();
                schedules.sort_by(|left, right| {
                    left.next_run_at
                        .cmp(&right.next_run_at)
                        .then_with(|| left.name.cmp(&right.name))
                });
                Ok(schedules)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        operation,
                        target_clients,
                        target_tags,
                        interval_secs,
                        catch_up_policy,
                        catch_up_limit,
                        retry_delay_secs,
                        max_failures,
                        failure_count,
                        last_error,
                        next_run_at::text AS next_run_at,
                        last_run_at::text AS last_run_at,
                        created_at::text AS created_at
                    FROM schedules
                    ORDER BY enabled DESC, next_run_at, name
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let operation: SqlJson<vpsman_common::JobCommand> =
                            row.try_get("operation")?;
                        Ok(schedule_view_from_row(ScheduleRowParts {
                            id: row.try_get("id")?,
                            name: row.try_get("name")?,
                            enabled: row.try_get("enabled")?,
                            operation: operation.0,
                            clients: row.try_get("target_clients")?,
                            tags: row.try_get("target_tags")?,
                            interval_secs: row.try_get("interval_secs")?,
                            catch_up_policy: row.try_get("catch_up_policy")?,
                            catch_up_limit: row.try_get("catch_up_limit")?,
                            retry_delay_secs: row.try_get("retry_delay_secs")?,
                            max_failures: row.try_get("max_failures")?,
                            failure_count: row.try_get("failure_count")?,
                            last_error: row.try_get("last_error")?,
                            next_run_at: row.try_get("next_run_at")?,
                            last_run_at: row.try_get("last_run_at")?,
                            created_at: row.try_get("created_at")?,
                        }))
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn create_schedule(
        &self,
        request: CreateScheduleRequest,
        operator: &AuthContext,
    ) -> Result<ScheduleView> {
        let id = Uuid::new_v4();
        let now = unix_now();
        let next_run_unix = request
            .start_at_unix
            .unwrap_or_else(|| now.saturating_add(request.interval_secs.max(1)));
        match self {
            Self::Memory(memory) => {
                let created_at = now.to_string();
                let schedule = schedule_view_from_row(ScheduleRowParts {
                    id,
                    name: request.name,
                    enabled: request.enabled,
                    operation: request.operation,
                    clients: request.clients,
                    tags: request.tags,
                    interval_secs: request.interval_secs as i64,
                    catch_up_policy: request.catch_up_policy,
                    catch_up_limit: request.catch_up_limit,
                    retry_delay_secs: request.retry_delay_secs,
                    max_failures: request.max_failures,
                    failure_count: 0,
                    last_error: None,
                    next_run_at: next_run_unix.to_string(),
                    last_run_at: None,
                    created_at,
                });
                let mut schedules = memory.schedules.write().await;
                if let Some(existing) = schedules
                    .iter_mut()
                    .find(|existing| existing.name == schedule.name)
                {
                    *existing = schedule.clone();
                } else {
                    schedules.push(schedule.clone());
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "schedule.upserted".to_string(),
                    target: format!("schedule:{}", schedule.id),
                    command_hash: None,
                    metadata: serde_json::json!({
                        "name": &schedule.name,
                        "operation_type": &schedule.command_type,
                        "clients": &schedule.clients,
                        "tags": &schedule.tags,
                        "interval_secs": schedule.interval_secs,
                        "catch_up_policy": &schedule.catch_up_policy,
                        "catch_up_limit": schedule.catch_up_limit,
                        "retry_delay_secs": schedule.retry_delay_secs,
                        "max_failures": schedule.max_failures,
                        "enabled": schedule.enabled,
                        "operator_username": &operator.operator.username,
                        "session_id": operator.session_id,
                    }),
                    created_at: now.to_string(),
                });
                Ok(schedule)
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    INSERT INTO schedules (
                        id,
                        actor_id,
                        name,
                        enabled,
                        operation,
                        target_clients,
                        target_tags,
                        interval_secs,
                        catch_up_policy,
                        catch_up_limit,
                        retry_delay_secs,
                        max_failures,
                        next_run_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, to_timestamp($13))
                    ON CONFLICT (name) DO UPDATE SET
                        actor_id = EXCLUDED.actor_id,
                        enabled = EXCLUDED.enabled,
                        operation = EXCLUDED.operation,
                        target_clients = EXCLUDED.target_clients,
                        target_tags = EXCLUDED.target_tags,
                        interval_secs = EXCLUDED.interval_secs,
                        catch_up_policy = EXCLUDED.catch_up_policy,
                        catch_up_limit = EXCLUDED.catch_up_limit,
                        retry_delay_secs = EXCLUDED.retry_delay_secs,
                        max_failures = EXCLUDED.max_failures,
                        failure_count = 0,
                        last_error = NULL,
                        next_run_at = EXCLUDED.next_run_at,
                        updated_at = now()
                    RETURNING
                        id,
                        name,
                        enabled,
                        operation,
                        target_clients,
                        target_tags,
                        interval_secs,
                        catch_up_policy,
                        catch_up_limit,
                        retry_delay_secs,
                        max_failures,
                        failure_count,
                        last_error,
                        next_run_at::text AS next_run_at,
                        last_run_at::text AS last_run_at,
                        created_at::text AS created_at
                    "#,
                )
                .bind(id)
                .bind(operator.operator.id)
                .bind(&request.name)
                .bind(request.enabled)
                .bind(SqlJson(request.operation.clone()))
                .bind(&request.clients)
                .bind(&request.tags)
                .bind(request.interval_secs as i64)
                .bind(&request.catch_up_policy)
                .bind(request.catch_up_limit)
                .bind(request.retry_delay_secs)
                .bind(request.max_failures)
                .bind(next_run_unix as f64)
                .fetch_one(pool)
                .await?;
                let operation: SqlJson<vpsman_common::JobCommand> = row.try_get("operation")?;
                let schedule = schedule_view_from_row(ScheduleRowParts {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    enabled: row.try_get("enabled")?,
                    operation: operation.0,
                    clients: row.try_get("target_clients")?,
                    tags: row.try_get("target_tags")?,
                    interval_secs: row.try_get("interval_secs")?,
                    catch_up_policy: row.try_get("catch_up_policy")?,
                    catch_up_limit: row.try_get("catch_up_limit")?,
                    retry_delay_secs: row.try_get("retry_delay_secs")?,
                    max_failures: row.try_get("max_failures")?,
                    failure_count: row.try_get("failure_count")?,
                    last_error: row.try_get("last_error")?,
                    next_run_at: row.try_get("next_run_at")?,
                    last_run_at: row.try_get("last_run_at")?,
                    created_at: row.try_get("created_at")?,
                });
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
                .bind("schedule.upserted")
                .bind(format!("schedule:{}", schedule.id))
                .bind(serde_json::json!({
                    "name": &schedule.name,
                    "operation_type": &schedule.command_type,
                    "clients": &schedule.clients,
                    "tags": &schedule.tags,
                    "interval_secs": schedule.interval_secs,
                    "catch_up_policy": &schedule.catch_up_policy,
                    "catch_up_limit": schedule.catch_up_limit,
                    "retry_delay_secs": schedule.retry_delay_secs,
                    "max_failures": schedule.max_failures,
                    "enabled": schedule.enabled,
                    "operator_username": &operator.operator.username,
                    "session_id": operator.session_id,
                }))
                .execute(pool)
                .await?;
                Ok(schedule)
            }
        }
    }
}

struct ScheduleRowParts {
    id: Uuid,
    name: String,
    enabled: bool,
    operation: vpsman_common::JobCommand,
    clients: Vec<String>,
    tags: Vec<String>,
    interval_secs: i64,
    catch_up_policy: String,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
    failure_count: i32,
    last_error: Option<String>,
    next_run_at: String,
    last_run_at: Option<String>,
    created_at: String,
}

fn schedule_view_from_row(parts: ScheduleRowParts) -> ScheduleView {
    let command_type = job_command_type_label(&parts.operation).to_string();
    ScheduleView {
        id: parts.id,
        name: parts.name,
        enabled: parts.enabled,
        command_type,
        operation: parts.operation,
        clients: parts.clients,
        tags: parts.tags,
        interval_secs: parts.interval_secs,
        catch_up_policy: parts.catch_up_policy,
        catch_up_limit: parts.catch_up_limit,
        retry_delay_secs: parts.retry_delay_secs,
        max_failures: parts.max_failures,
        failure_count: parts.failure_count,
        last_error: parts.last_error,
        next_run_at: parts.next_run_at,
        last_run_at: parts.last_run_at,
        created_at: parts.created_at,
    }
}
