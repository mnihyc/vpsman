use std::cmp::Ordering;

use anyhow::Result;
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
        "enabled" | "state" => left.enabled.cmp(&right.enabled),
        "interval_secs" | "interval" => left.interval_secs.cmp(&right.interval_secs),
        "command_type" | "operation" => left.command_type.cmp(&right.command_type),
        "targets" => left.selector_expression.cmp(&right.selector_expression),
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
}

fn schedule_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort, descending) {
        (None, _) => "enabled DESC, next_run_at ASC, name ASC",
        (Some("created_at"), true) => "created_at DESC, id DESC",
        (Some("created_at"), false) => "created_at ASC, id ASC",
        (Some("enabled" | "state"), true) => "enabled DESC, next_run_at ASC, id DESC",
        (Some("enabled" | "state"), false) => "enabled ASC, next_run_at ASC, id ASC",
        (Some("interval_secs" | "interval"), true) => "interval_secs DESC, id DESC",
        (Some("interval_secs" | "interval"), false) => "interval_secs ASC, id ASC",
        (Some("name"), true) => "name DESC, id DESC",
        (Some("name"), false) => "name ASC, id ASC",
        (Some("targets"), true) => "selector_expression DESC, id DESC",
        (Some("targets"), false) => "selector_expression ASC, id ASC",
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
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR name ILIKE $3 ESCAPE '\'
                        OR operation::text ILIKE $3 ESCAPE '\'
                        OR selector_expression ILIKE $3 ESCAPE '\'
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
                    selector_expression: request.selector_expression,
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
                        "selector_expression": &schedule.selector_expression,
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
                        selector_expression,
                        interval_secs,
                        catch_up_policy,
                        catch_up_limit,
                        retry_delay_secs,
                        max_failures,
                        next_run_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, to_timestamp($12))
                    ON CONFLICT (name) DO UPDATE SET
                        actor_id = EXCLUDED.actor_id,
                        enabled = EXCLUDED.enabled,
                        operation = EXCLUDED.operation,
                        selector_expression = EXCLUDED.selector_expression,
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
                        selector_expression,
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
                .bind(request.selector_expression.trim())
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
                    selector_expression: row.try_get("selector_expression")?,
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
                    "selector_expression": &schedule.selector_expression,
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
    selector_expression: String,
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
        selector_expression: parts.selector_expression,
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
