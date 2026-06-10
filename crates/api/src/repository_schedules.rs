use std::cmp::Ordering;
use std::str::FromStr;

use anyhow::Result;
use chrono::{DateTime, Utc};
use croner::Cron;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;

use crate::job_request::job_command_type_label;
use crate::model::*;
use crate::repository::Repository;
use crate::unix_now;
use crate::util::{limit_or_default, offset_or_default, search_pattern, sort_descending};

fn compare_text_or_number(left: &str, right: &str) -> Ordering {
    match (left.parse::<i128>(), right.parse::<i128>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn compare_schedule(left: &ScheduleView, right: &ScheduleView, sort: Option<&str>) -> Ordering {
    match sort.unwrap_or("next_run_at") {
        "created_at" => compare_text_or_number(&left.created_at, &right.created_at),
        "updated_at" => compare_text_or_number(&left.updated_at, &right.updated_at),
        "enabled" | "state" => left.enabled.cmp(&right.enabled),
        "cron_expr" | "cron" => left.cron_expr.cmp(&right.cron_expr),
        "command_type" | "operation" => left.command_type.cmp(&right.command_type),
        "targets" => left
            .target_client_ids
            .len()
            .cmp(&right.target_client_ids.len())
            .then_with(|| left.selector_expression.cmp(&right.selector_expression)),
        "failures" | "failure_count" => left.failure_count.cmp(&right.failure_count),
        "name" => left.name.cmp(&right.name),
        _ => compare_text_or_number(&left.next_run_at, &right.next_run_at),
    }
}

fn schedule_matches_search(schedule: &ScheduleView, needle: &str) -> bool {
    schedule
        .id
        .to_string()
        .to_ascii_lowercase()
        .contains(needle)
        || schedule.name.to_ascii_lowercase().contains(needle)
        || schedule.command_type.to_ascii_lowercase().contains(needle)
        || schedule.cron_expr.to_ascii_lowercase().contains(needle)
        || schedule
            .catch_up_policy
            .to_ascii_lowercase()
            .contains(needle)
        || schedule
            .last_error
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || schedule
            .selector_expression
            .to_ascii_lowercase()
            .contains(needle)
        || schedule
            .target_client_ids
            .iter()
            .any(|id| id.to_ascii_lowercase().contains(needle))
}

fn schedule_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort, descending) {
        (None, _) => "enabled DESC, next_run_at ASC, name ASC",
        (Some("created_at"), true) => "created_at DESC, id DESC",
        (Some("created_at"), false) => "created_at ASC, id ASC",
        (Some("enabled" | "state"), true) => "enabled DESC, next_run_at ASC, id DESC",
        (Some("enabled" | "state"), false) => "enabled ASC, next_run_at ASC, id ASC",
        (Some("cron_expr" | "cron"), true) => "cron_expr DESC, id DESC",
        (Some("cron_expr" | "cron"), false) => "cron_expr ASC, id ASC",
        (Some("name"), true) => "name DESC, id DESC",
        (Some("name"), false) => "name ASC, id ASC",
        (Some("targets"), true) => {
            "cardinality(target_client_ids) DESC, selector_expression DESC, id DESC"
        }
        (Some("targets"), false) => {
            "cardinality(target_client_ids) ASC, selector_expression ASC, id ASC"
        }
        (Some("failures" | "failure_count"), true) => "failure_count DESC, id DESC",
        (Some("failures" | "failure_count"), false) => "failure_count ASC, id ASC",
        (_, true) => "next_run_at DESC, id DESC",
        (_, false) => "next_run_at ASC, id ASC",
    }
}

impl Repository {
    pub(crate) async fn list_schedules(&self) -> Result<Vec<ScheduleView>> {
        self.query_schedules(&ListQuery::default()).await
    }

    pub(crate) async fn query_schedules(&self, query: &ListQuery) -> Result<Vec<ScheduleView>> {
        let limit = query.limit.map(|limit| limit_or_default(Some(limit)));
        let offset = offset_or_default(query.offset);
        let q = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match self {
            Self::Memory(memory) => {
                let q = q.map(|value| value.to_ascii_lowercase());
                let mut schedules = memory
                    .schedules
                    .read()
                    .await
                    .iter()
                    .filter(|schedule| schedule.deleted_at.is_none())
                    .filter(|schedule| {
                        q.as_deref()
                            .map(|needle| schedule_matches_search(schedule, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                schedules.sort_by(|left, right| {
                    if query.sort.is_some() {
                        let descending = sort_descending(query.dir.as_deref(), false);
                        let ordering = compare_schedule(left, right, query.sort.as_deref())
                            .then_with(|| left.name.cmp(&right.name))
                            .then_with(|| left.id.cmp(&right.id));
                        if descending {
                            ordering.reverse()
                        } else {
                            ordering
                        }
                    } else {
                        right
                            .enabled
                            .cmp(&left.enabled)
                            .then_with(|| {
                                compare_text_or_number(&left.next_run_at, &right.next_run_at)
                            })
                            .then_with(|| left.name.cmp(&right.name))
                    }
                });
                Ok(schedules
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit.unwrap_or(i64::MAX) as usize)
                    .collect())
            }
            Self::Postgres(pool) => {
                let order_by = schedule_order_by(
                    query.sort.as_deref(),
                    sort_descending(query.dir.as_deref(), false),
                );
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        operation,
                        selector_expression,
                        target_client_ids,
                        cron_expr,
                        timezone,
                        catch_up_policy,
                        catch_up_limit,
                        retry_delay_secs,
                        max_failures,
                        failure_count,
                        last_error,
                        next_run_at::text AS next_run_at,
                        last_run_at::text AS last_run_at,
                        deferred_until::text AS deferred_until,
                        deleted_at::text AS deleted_at,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM schedules
                    WHERE deleted_at IS NULL
                      AND (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR name ILIKE $3 ESCAPE '\'
                        OR operation::text ILIKE $3 ESCAPE '\'
                        OR selector_expression ILIKE $3 ESCAPE '\'
                        OR target_client_ids::text ILIKE $3 ESCAPE '\'
                        OR cron_expr ILIKE $3 ESCAPE '\'
                        OR catch_up_policy ILIKE $3 ESCAPE '\'
                        OR last_error ILIKE $3 ESCAPE '\'
                    )
                    ORDER BY {order_by}
                    LIMIT $1
                    OFFSET $2
                    "#,
                ))
                .bind(limit.unwrap_or(1000))
                .bind(offset)
                .bind(search_pattern(&query.q))
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
                            selector_expression: row.try_get("selector_expression")?,
                            target_client_ids: row.try_get("target_client_ids")?,
                            cron_expr: row.try_get("cron_expr")?,
                            timezone: row.try_get("timezone")?,
                            catch_up_policy: row.try_get("catch_up_policy")?,
                            catch_up_limit: row.try_get("catch_up_limit")?,
                            retry_delay_secs: row.try_get("retry_delay_secs")?,
                            max_failures: row.try_get("max_failures")?,
                            failure_count: row.try_get("failure_count")?,
                            last_error: row.try_get("last_error")?,
                            next_run_at: row.try_get("next_run_at")?,
                            last_run_at: row.try_get("last_run_at")?,
                            deferred_until: row.try_get("deferred_until")?,
                            deleted_at: row.try_get("deleted_at")?,
                            created_at: row.try_get("created_at")?,
                            updated_at: row.try_get("updated_at")?,
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
        let CreateScheduleRequest {
            name,
            operation,
            selector_expression,
            target_client_ids,
            cron_expr,
            timezone,
            enabled,
            catch_up_policy,
            catch_up_limit,
            retry_delay_secs,
            max_failures,
            ..
        } = request;
        self.create_schedule_record(
            ScheduleCreateInput {
                name,
                operation,
                selector_expression,
                target_client_ids,
                cron_expr,
                timezone,
                enabled,
                catch_up_policy,
                catch_up_limit,
                retry_delay_secs,
                max_failures,
            },
            operator,
        )
        .await
    }

    pub(crate) async fn create_schedule_record(
        &self,
        request: ScheduleCreateInput,
        operator: &AuthContext,
    ) -> Result<ScheduleView> {
        let id = Uuid::new_v4();
        let now = unix_now();
        let next_runs = next_cron_runs(&request.cron_expr, 5)?;
        let next_run = next_runs.first().cloned().unwrap_or_else(|| {
            DateTime::<Utc>::from_timestamp(now as i64, 0)
                .unwrap()
                .to_rfc3339()
        });
        match self {
            Self::Memory(memory) => {
                let created_at = now.to_string();
                let schedule = schedule_view_from_row(ScheduleRowParts {
                    id,
                    name: request.name,
                    enabled: request.enabled,
                    operation: request.operation,
                    selector_expression: request.selector_expression,
                    target_client_ids: request.target_client_ids,
                    cron_expr: request.cron_expr,
                    timezone: request.timezone,
                    catch_up_policy: request.catch_up_policy,
                    catch_up_limit: request.catch_up_limit,
                    retry_delay_secs: request.retry_delay_secs,
                    max_failures: request.max_failures,
                    failure_count: 0,
                    last_error: None,
                    next_run_at: next_run,
                    last_run_at: None,
                    deferred_until: None,
                    deleted_at: None,
                    created_at: created_at.clone(),
                    updated_at: created_at,
                });
                let mut schedules = memory.schedules.write().await;
                schedules.push(schedule.clone());
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "schedule.created".to_string(),
                    target: format!("schedule:{}", schedule.id),
                    command_hash: None,
                    metadata: serde_json::json!({
                        "name": &schedule.name,
                        "operation_type": &schedule.command_type,
                        "selector_expression": &schedule.selector_expression,
                        "target_client_ids": &schedule.target_client_ids,
                        "target_count": schedule.target_client_ids.len(),
                        "cron_expr": &schedule.cron_expr,
                        "timezone": &schedule.timezone,
                        "next_runs": &schedule.next_runs,
                        "catch_up_policy": &schedule.catch_up_policy,
                        "catch_up_limit": schedule.catch_up_limit,
                        "retry_delay_secs": schedule.retry_delay_secs,
                        "max_failures": schedule.max_failures,
                        "enabled": schedule.enabled,
                        "deferred_until": schedule.deferred_until,
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
                        selector_expression,
                        target_client_ids,
                        cron_expr,
                        timezone,
                        catch_up_policy,
                        catch_up_limit,
                        retry_delay_secs,
                        max_failures,
                        next_run_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, to_timestamp($14))
                    RETURNING
                        id,
                        name,
                        enabled,
                        operation,
                        selector_expression,
                        target_client_ids,
                        cron_expr,
                        timezone,
                        catch_up_policy,
                        catch_up_limit,
                        retry_delay_secs,
                        max_failures,
                        failure_count,
                        last_error,
                        next_run_at::text AS next_run_at,
                        last_run_at::text AS last_run_at,
                        deferred_until::text AS deferred_until,
                        deleted_at::text AS deleted_at,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(id)
                .bind(operator.operator.id)
                .bind(&request.name)
                .bind(request.enabled)
                .bind(SqlJson(request.operation.clone()))
                .bind(request.selector_expression.trim())
                .bind(&request.target_client_ids)
                .bind(&request.cron_expr)
                .bind(&request.timezone)
                .bind(&request.catch_up_policy)
                .bind(request.catch_up_limit)
                .bind(request.retry_delay_secs)
                .bind(request.max_failures)
                .bind(next_run_timestamp(&next_run)? as f64)
                .fetch_one(pool)
                .await?;
                let operation: SqlJson<vpsman_common::JobCommand> = row.try_get("operation")?;
                let schedule = schedule_view_from_row(ScheduleRowParts {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    enabled: row.try_get("enabled")?,
                    operation: operation.0,
                    selector_expression: row.try_get("selector_expression")?,
                    target_client_ids: row.try_get("target_client_ids")?,
                    cron_expr: row.try_get("cron_expr")?,
                    timezone: row.try_get("timezone")?,
                    catch_up_policy: row.try_get("catch_up_policy")?,
                    catch_up_limit: row.try_get("catch_up_limit")?,
                    retry_delay_secs: row.try_get("retry_delay_secs")?,
                    max_failures: row.try_get("max_failures")?,
                    failure_count: row.try_get("failure_count")?,
                    last_error: row.try_get("last_error")?,
                    next_run_at: row.try_get("next_run_at")?,
                    last_run_at: row.try_get("last_run_at")?,
                    deferred_until: row.try_get("deferred_until")?,
                    deleted_at: row.try_get("deleted_at")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
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
                .bind("schedule.created")
                .bind(format!("schedule:{}", schedule.id))
                .bind(serde_json::json!({
                    "name": &schedule.name,
                    "operation_type": &schedule.command_type,
                    "selector_expression": &schedule.selector_expression,
                    "target_client_ids": &schedule.target_client_ids,
                    "target_count": schedule.target_client_ids.len(),
                    "cron_expr": &schedule.cron_expr,
                    "timezone": &schedule.timezone,
                    "next_runs": &schedule.next_runs,
                    "catch_up_policy": &schedule.catch_up_policy,
                    "catch_up_limit": schedule.catch_up_limit,
                    "retry_delay_secs": schedule.retry_delay_secs,
                    "max_failures": schedule.max_failures,
                    "enabled": schedule.enabled,
                    "deferred_until": schedule.deferred_until,
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

impl Repository {
    pub(crate) async fn schedule_by_id(&self, schedule_id: Uuid) -> Result<ScheduleView> {
        match self {
            Self::Memory(memory) => memory
                .schedules
                .read()
                .await
                .iter()
                .find(|schedule| schedule.id == schedule_id && schedule.deleted_at.is_none())
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("schedule_not_found:{schedule_id}")),
            Self::Postgres(pool) => {
                let sql = schedule_select_sql("WHERE id = $1 AND deleted_at IS NULL");
                let row = sqlx::query(&sql).bind(schedule_id).fetch_one(pool).await?;
                schedule_from_postgres_row(row)
            }
        }
    }

    pub(crate) async fn update_schedule_record(
        &self,
        schedule_id: Uuid,
        request: ScheduleCreateInput,
        operator: &AuthContext,
    ) -> Result<ScheduleView> {
        let now = unix_now().to_string();
        let next_runs = next_cron_runs(&request.cron_expr, 5)?;
        let next_run = next_runs.first().cloned().unwrap_or_else(|| now.clone());
        match self {
            Self::Memory(memory) => {
                let mut schedules = memory.schedules.write().await;
                let schedule = schedules
                    .iter_mut()
                    .find(|schedule| schedule.id == schedule_id && schedule.deleted_at.is_none())
                    .ok_or_else(|| anyhow::anyhow!("schedule_not_found:{schedule_id}"))?;
                schedule.name = request.name;
                schedule.enabled = request.enabled;
                schedule.command_type = job_command_type_label(&request.operation).to_string();
                schedule.operation = request.operation;
                schedule.selector_expression = request.selector_expression;
                schedule.target_client_ids = request.target_client_ids;
                schedule.cron_expr = request.cron_expr;
                schedule.timezone = request.timezone;
                schedule.next_runs = next_runs;
                schedule.catch_up_policy = request.catch_up_policy;
                schedule.catch_up_limit = request.catch_up_limit;
                schedule.retry_delay_secs = request.retry_delay_secs;
                schedule.max_failures = request.max_failures;
                schedule.failure_count = 0;
                schedule.last_error = None;
                schedule.next_run_at = next_run;
                schedule.updated_at = now.clone();
                let schedule = schedule.clone();
                drop(schedules);
                record_memory_schedule_audit(memory, &schedule, operator, "schedule.updated").await;
                Ok(schedule)
            }
            Self::Postgres(pool) => {
                let result = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET
                        actor_id = $2,
                        name = $3,
                        enabled = $4,
                        operation = $5,
                        selector_expression = $6,
                        target_client_ids = $7,
                        cron_expr = $8,
                        timezone = $9,
                        catch_up_policy = $10,
                        catch_up_limit = $11,
                        retry_delay_secs = $12,
                        max_failures = $13,
                        next_run_at = to_timestamp($14),
                        failure_count = 0,
                        last_error = NULL,
                        updated_at = now()
                    WHERE id = $1 AND deleted_at IS NULL
                    "#,
                )
                .bind(schedule_id)
                .bind(operator.operator.id)
                .bind(&request.name)
                .bind(request.enabled)
                .bind(SqlJson(request.operation))
                .bind(request.selector_expression.trim())
                .bind(&request.target_client_ids)
                .bind(&request.cron_expr)
                .bind(&request.timezone)
                .bind(&request.catch_up_policy)
                .bind(request.catch_up_limit)
                .bind(request.retry_delay_secs)
                .bind(request.max_failures)
                .bind(next_run_timestamp(&next_run)? as f64)
                .execute(pool)
                .await?;
                anyhow::ensure!(
                    result.rows_affected() > 0,
                    "schedule_not_found:{schedule_id}"
                );
                let schedule = self.schedule_by_id(schedule_id).await?;
                record_postgres_schedule_audit(pool, &schedule, operator, "schedule.updated")
                    .await?;
                Ok(schedule)
            }
        }
    }

    pub(crate) async fn update_schedule_targets(
        &self,
        schedule_id: Uuid,
        selector_expression: String,
        target_client_ids: Vec<String>,
        operator: &AuthContext,
    ) -> Result<ScheduleView> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut schedules = memory.schedules.write().await;
                let schedule = schedules
                    .iter_mut()
                    .find(|schedule| schedule.id == schedule_id && schedule.deleted_at.is_none())
                    .ok_or_else(|| anyhow::anyhow!("schedule_not_found:{schedule_id}"))?;
                schedule.selector_expression = selector_expression;
                schedule.target_client_ids = target_client_ids;
                schedule.updated_at = now;
                let schedule = schedule.clone();
                drop(schedules);
                record_memory_schedule_audit(
                    memory,
                    &schedule,
                    operator,
                    "schedule.targets_updated",
                )
                .await;
                Ok(schedule)
            }
            Self::Postgres(pool) => {
                let result = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET
                        actor_id = $2,
                        selector_expression = $3,
                        target_client_ids = $4,
                        updated_at = now()
                    WHERE id = $1 AND deleted_at IS NULL
                    "#,
                )
                .bind(schedule_id)
                .bind(operator.operator.id)
                .bind(selector_expression.trim())
                .bind(&target_client_ids)
                .execute(pool)
                .await?;
                anyhow::ensure!(
                    result.rows_affected() > 0,
                    "schedule_not_found:{schedule_id}"
                );
                let schedule = self.schedule_by_id(schedule_id).await?;
                record_postgres_schedule_audit(
                    pool,
                    &schedule,
                    operator,
                    "schedule.targets_updated",
                )
                .await?;
                Ok(schedule)
            }
        }
    }

    pub(crate) async fn set_schedule_enabled(
        &self,
        schedule_id: Uuid,
        enabled: bool,
        operator: &AuthContext,
    ) -> Result<ScheduleView> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut schedules = memory.schedules.write().await;
                let schedule = schedules
                    .iter_mut()
                    .find(|schedule| schedule.id == schedule_id && schedule.deleted_at.is_none())
                    .ok_or_else(|| anyhow::anyhow!("schedule_not_found:{schedule_id}"))?;
                schedule.enabled = enabled;
                schedule.updated_at = now;
                let schedule = schedule.clone();
                drop(schedules);
                record_memory_schedule_audit(
                    memory,
                    &schedule,
                    operator,
                    if enabled {
                        "schedule.enabled"
                    } else {
                        "schedule.disabled"
                    },
                )
                .await;
                Ok(schedule)
            }
            Self::Postgres(pool) => {
                let result = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET enabled = $2, actor_id = $3, updated_at = now()
                    WHERE id = $1 AND deleted_at IS NULL
                    "#,
                )
                .bind(schedule_id)
                .bind(enabled)
                .bind(operator.operator.id)
                .execute(pool)
                .await?;
                anyhow::ensure!(
                    result.rows_affected() > 0,
                    "schedule_not_found:{schedule_id}"
                );
                let schedule = self.schedule_by_id(schedule_id).await?;
                record_postgres_schedule_audit(
                    pool,
                    &schedule,
                    operator,
                    if enabled {
                        "schedule.enabled"
                    } else {
                        "schedule.disabled"
                    },
                )
                .await?;
                Ok(schedule)
            }
        }
    }

    pub(crate) async fn defer_schedule(
        &self,
        schedule_id: Uuid,
        deferred_until: &str,
        reason: Option<&str>,
        operator: &AuthContext,
    ) -> Result<ScheduleView> {
        let _ = next_run_timestamp(deferred_until)?;
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut schedules = memory.schedules.write().await;
                let schedule = schedules
                    .iter_mut()
                    .find(|schedule| schedule.id == schedule_id && schedule.deleted_at.is_none())
                    .ok_or_else(|| anyhow::anyhow!("schedule_not_found:{schedule_id}"))?;
                schedule.deferred_until = Some(deferred_until.to_string());
                schedule.updated_at = now;
                let schedule = schedule.clone();
                drop(schedules);
                record_memory_schedule_audit_with_extra(
                    memory,
                    &schedule,
                    operator,
                    "schedule.deferred",
                    serde_json::json!({ "reason": reason }),
                )
                .await;
                Ok(schedule)
            }
            Self::Postgres(pool) => {
                let result = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET deferred_until = to_timestamp($2), actor_id = $3, updated_at = now()
                    WHERE id = $1 AND deleted_at IS NULL
                    "#,
                )
                .bind(schedule_id)
                .bind(next_run_timestamp(deferred_until)? as f64)
                .bind(operator.operator.id)
                .execute(pool)
                .await?;
                anyhow::ensure!(
                    result.rows_affected() > 0,
                    "schedule_not_found:{schedule_id}"
                );
                let schedule = self.schedule_by_id(schedule_id).await?;
                record_postgres_schedule_audit_with_extra(
                    pool,
                    &schedule,
                    operator,
                    "schedule.deferred",
                    serde_json::json!({ "reason": reason }),
                )
                .await?;
                Ok(schedule)
            }
        }
    }

    pub(crate) async fn soft_delete_schedule(
        &self,
        schedule_id: Uuid,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut schedules = memory.schedules.write().await;
                let schedule = schedules
                    .iter_mut()
                    .find(|schedule| schedule.id == schedule_id && schedule.deleted_at.is_none())
                    .ok_or_else(|| anyhow::anyhow!("schedule_not_found:{schedule_id}"))?;
                schedule.deleted_at = Some(now.clone());
                schedule.updated_at = now;
                let schedule = schedule.clone();
                drop(schedules);
                record_memory_schedule_audit(memory, &schedule, operator, "schedule.deleted").await;
                Ok(())
            }
            Self::Postgres(pool) => {
                let schedule = self.schedule_by_id(schedule_id).await?;
                let result = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET deleted_at = now(), deleted_by = $2, actor_id = $2, enabled = FALSE, updated_at = now()
                    WHERE id = $1 AND deleted_at IS NULL
                    "#,
                )
                .bind(schedule_id)
                .bind(operator.operator.id)
                .execute(pool)
                .await?;
                anyhow::ensure!(
                    result.rows_affected() > 0,
                    "schedule_not_found:{schedule_id}"
                );
                record_postgres_schedule_audit(pool, &schedule, operator, "schedule.deleted")
                    .await?;
                Ok(())
            }
        }
    }
}

pub(crate) struct ScheduleCreateInput {
    pub(crate) name: String,
    pub(crate) operation: vpsman_common::JobCommand,
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) cron_expr: String,
    pub(crate) timezone: String,
    pub(crate) enabled: bool,
    pub(crate) catch_up_policy: String,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
}

struct ScheduleRowParts {
    id: Uuid,
    name: String,
    enabled: bool,
    operation: vpsman_common::JobCommand,
    selector_expression: String,
    target_client_ids: Vec<String>,
    cron_expr: String,
    timezone: String,
    catch_up_policy: String,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
    failure_count: i32,
    last_error: Option<String>,
    next_run_at: String,
    last_run_at: Option<String>,
    deferred_until: Option<String>,
    deleted_at: Option<String>,
    created_at: String,
    updated_at: String,
}

fn schedule_view_from_row(parts: ScheduleRowParts) -> ScheduleView {
    let command_type = job_command_type_label(&parts.operation).to_string();
    ScheduleView {
        id: parts.id,
        name: parts.name,
        enabled: parts.enabled,
        command_type,
        operation: parts.operation,
        selector_expression: parts.selector_expression,
        target_client_ids: parts.target_client_ids,
        next_runs: next_cron_runs(&parts.cron_expr, 5).unwrap_or_default(),
        cron_expr: parts.cron_expr,
        timezone: parts.timezone,
        catch_up_policy: parts.catch_up_policy,
        catch_up_limit: parts.catch_up_limit,
        retry_delay_secs: parts.retry_delay_secs,
        max_failures: parts.max_failures,
        failure_count: parts.failure_count,
        last_error: parts.last_error,
        next_run_at: parts.next_run_at,
        last_run_at: parts.last_run_at,
        deferred_until: parts.deferred_until,
        deleted_at: parts.deleted_at,
        created_at: parts.created_at,
        updated_at: parts.updated_at,
    }
}

fn schedule_select_sql(where_clause: &str) -> String {
    format!(
        r#"
        SELECT
            id,
            name,
            enabled,
            operation,
            selector_expression,
            target_client_ids,
            cron_expr,
            timezone,
            catch_up_policy,
            catch_up_limit,
            retry_delay_secs,
            max_failures,
            failure_count,
            last_error,
            next_run_at::text AS next_run_at,
            last_run_at::text AS last_run_at,
            deferred_until::text AS deferred_until,
            deleted_at::text AS deleted_at,
            created_at::text AS created_at,
            updated_at::text AS updated_at
        FROM schedules
        {where_clause}
        "#
    )
}

fn schedule_from_postgres_row(row: sqlx::postgres::PgRow) -> Result<ScheduleView> {
    let operation: SqlJson<vpsman_common::JobCommand> = row.try_get("operation")?;
    Ok(schedule_view_from_row(ScheduleRowParts {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        enabled: row.try_get("enabled")?,
        operation: operation.0,
        selector_expression: row.try_get("selector_expression")?,
        target_client_ids: row.try_get("target_client_ids")?,
        cron_expr: row.try_get("cron_expr")?,
        timezone: row.try_get("timezone")?,
        catch_up_policy: row.try_get("catch_up_policy")?,
        catch_up_limit: row.try_get("catch_up_limit")?,
        retry_delay_secs: row.try_get("retry_delay_secs")?,
        max_failures: row.try_get("max_failures")?,
        failure_count: row.try_get("failure_count")?,
        last_error: row.try_get("last_error")?,
        next_run_at: row.try_get("next_run_at")?,
        last_run_at: row.try_get("last_run_at")?,
        deferred_until: row.try_get("deferred_until")?,
        deleted_at: row.try_get("deleted_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    }))
}

async fn record_memory_schedule_audit(
    memory: &crate::repository::MemoryState,
    schedule: &ScheduleView,
    operator: &AuthContext,
    action: &str,
) {
    record_memory_schedule_audit_with_extra(
        memory,
        schedule,
        operator,
        action,
        serde_json::Value::Null,
    )
    .await;
}

async fn record_memory_schedule_audit_with_extra(
    memory: &crate::repository::MemoryState,
    schedule: &ScheduleView,
    operator: &AuthContext,
    action: &str,
    extra: serde_json::Value,
) {
    memory.audits.write().await.push(AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: format!("schedule:{}", schedule.id),
        command_hash: None,
        metadata: schedule_audit_metadata(schedule, operator, extra),
        created_at: unix_now().to_string(),
    });
}

async fn record_postgres_schedule_audit(
    pool: &sqlx::PgPool,
    schedule: &ScheduleView,
    operator: &AuthContext,
    action: &str,
) -> Result<()> {
    record_postgres_schedule_audit_with_extra(
        pool,
        schedule,
        operator,
        action,
        serde_json::Value::Null,
    )
    .await
}

async fn record_postgres_schedule_audit_with_extra(
    pool: &sqlx::PgPool,
    schedule: &ScheduleView,
    operator: &AuthContext,
    action: &str,
    extra: serde_json::Value,
) -> Result<()> {
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
    .bind(action)
    .bind(format!("schedule:{}", schedule.id))
    .bind(schedule_audit_metadata(schedule, operator, extra))
    .execute(pool)
    .await?;
    Ok(())
}

fn schedule_audit_metadata(
    schedule: &ScheduleView,
    operator: &AuthContext,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut metadata = serde_json::json!({
        "schedule_id": schedule.id,
        "name": &schedule.name,
        "operation_type": &schedule.command_type,
        "selector_expression": &schedule.selector_expression,
        "target_client_ids": &schedule.target_client_ids,
        "target_count": schedule.target_client_ids.len(),
        "cron_expr": &schedule.cron_expr,
        "timezone": &schedule.timezone,
        "next_runs": &schedule.next_runs,
        "catch_up_policy": &schedule.catch_up_policy,
        "catch_up_limit": schedule.catch_up_limit,
        "retry_delay_secs": schedule.retry_delay_secs,
        "max_failures": schedule.max_failures,
        "enabled": schedule.enabled,
        "deferred_until": schedule.deferred_until,
        "deleted_at": schedule.deleted_at,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    });
    if !extra.is_null() {
        metadata["extra"] = extra;
    }
    metadata
}

pub(crate) fn next_cron_runs(cron_expr: &str, count: usize) -> Result<Vec<String>> {
    let cron = Cron::from_str(cron_expr)?;
    Ok(cron
        .iter_after(Utc::now())
        .take(count)
        .map(|run| run.to_rfc3339())
        .collect())
}

fn next_run_timestamp(value: &str) -> Result<i64> {
    Ok(DateTime::parse_from_rfc3339(value)?.timestamp())
}
