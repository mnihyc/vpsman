use std::str::FromStr;
use std::{cmp::Ordering, collections::HashSet};

use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Utc};
use croner::Cron;
use serde_json::Value;
use sqlx::{types::Json as SqlJson, Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::{alert_event_argv_template_hash, encode_json, payload_hash, JobCommand};

use crate::job_request::job_command_type_label;
use crate::model::*;
use crate::repository::Repository;
use crate::unix_now;
use crate::util::{limit_or_default, offset_or_default, search_pattern, sort_descending};

const SCHEDULE_OPERATION_INVALID: &str = "schedule_operation_invalid";
const INVALID_SCHEDULE_COMMAND_TYPE: &str = "invalid_operation";
const ALERT_LIFECYCLE_ARM_LOCK: &str = "vpsman.alert_lifecycle_arm";

fn schedule_trigger_kind_storage(kind: ScheduleTriggerKind) -> &'static str {
    match kind {
        ScheduleTriggerKind::Cron => "cron",
        ScheduleTriggerKind::Event => "event",
    }
}

async fn lock_event_schedule_arm_boundary(tx: &mut Transaction<'_, Postgres>) -> Result<i64> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
        .bind(ALERT_LIFECYCLE_ARM_LOCK)
        .execute(&mut **tx)
        .await?;
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(max(event_seq), 0) FROM alert_lifecycle_events",
    )
    .fetch_one(&mut **tx)
    .await?)
}

fn decode_stored_schedule_operation(
    operation: Option<Value>,
    trigger_kind: ScheduleTriggerKind,
    event_argv_template: Option<&[String]>,
) -> (Option<JobCommand>, Option<String>, String) {
    if trigger_kind == ScheduleTriggerKind::Event {
        return match alert_event_argv_template_hash(event_argv_template) {
            Ok(operation_payload_hash) => (None, None, operation_payload_hash),
            Err(_) => (
                None,
                Some(SCHEDULE_OPERATION_INVALID.to_string()),
                payload_hash(b"invalid-event-template"),
            ),
        };
    }
    let Some(operation) = operation else {
        return (
            None,
            Some(SCHEDULE_OPERATION_INVALID.to_string()),
            payload_hash(b"null"),
        );
    };
    let operation_payload_hash = payload_hash(operation.to_string().as_bytes());
    match serde_json::from_value(operation) {
        Ok(operation) => (Some(operation), None, operation_payload_hash),
        Err(_) => (
            None,
            Some(SCHEDULE_OPERATION_INVALID.to_string()),
            operation_payload_hash,
        ),
    }
}

fn valid_schedule_operation_payload_hash(operation: &JobCommand) -> Result<String> {
    Ok(payload_hash(&encode_json(operation)?))
}

fn event_schedule_operation_payload_hash(event_argv_template: Option<&[String]>) -> Result<String> {
    alert_event_argv_template_hash(event_argv_template).map_err(anyhow::Error::msg)
}

fn ensure_schedule_operation_valid(schedule: &ScheduleView) -> Result<()> {
    if schedule.trigger_kind == ScheduleTriggerKind::Event {
        ensure!(
            schedule.operation_error.is_none(),
            SCHEDULE_OPERATION_INVALID
        );
        return Ok(());
    }
    ensure!(
        schedule.operation.is_some() && schedule.operation_error.is_none(),
        SCHEDULE_OPERATION_INVALID
    );
    Ok(())
}

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
        _ => left.next_run_at.cmp(&right.next_run_at),
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
            .cron_expr
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
        || schedule
            .event_expression
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
        || schedule
            .catch_up_policy
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
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
        (None, _) => "enabled DESC, next_run_at ASC, name ASC, id ASC",
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
        self.query_schedules_filtered(query, false).await
    }

    pub(crate) async fn query_backup_policy_schedules(
        &self,
        query: &ListQuery,
    ) -> Result<Vec<ScheduleView>> {
        self.query_schedules_filtered(query, true).await
    }

    async fn query_schedules_filtered(
        &self,
        query: &ListQuery,
        require_backup_policy_metadata: bool,
    ) -> Result<Vec<ScheduleView>> {
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
                let backup_policy_schedule_ids = if require_backup_policy_metadata {
                    Some(
                        memory
                            .backup_policies
                            .read()
                            .await
                            .iter()
                            .map(|policy| policy.schedule_id)
                            .collect::<HashSet<_>>(),
                    )
                } else {
                    None
                };
                let mut schedules = memory
                    .schedules
                    .read()
                    .await
                    .iter()
                    .filter(|schedule| schedule.deleted_at.is_none())
                    .filter(|schedule| {
                        backup_policy_schedule_ids.as_ref().is_none_or(|ids| {
                            ids.contains(&schedule.id)
                                && matches!(
                                    schedule.operation,
                                    Some(vpsman_common::JobCommand::Backup { .. })
                                )
                        })
                    })
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
                            .then_with(|| left.next_run_at.cmp(&right.next_run_at))
                            .then_with(|| left.name.cmp(&right.name))
                            .then_with(|| left.id.cmp(&right.id))
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
                        trigger_kind,
                        definition_revision,
                        operation,
                        event_argv_template,
                        selector_expression,
                        target_client_ids,
                        cron_expr,
                        event_expression,
                        event_armed_at::text AS event_armed_at,
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
                        OR event_argv_template::text ILIKE $3 ESCAPE '\'
                        OR selector_expression ILIKE $3 ESCAPE '\'
                        OR target_client_ids::text ILIKE $3 ESCAPE '\'
                        OR cron_expr ILIKE $3 ESCAPE '\'
                        OR event_expression ILIKE $3 ESCAPE '\'
                        OR catch_up_policy ILIKE $3 ESCAPE '\'
                        OR last_error ILIKE $3 ESCAPE '\'
                      )
                      AND (
                        $4::boolean = FALSE
                        OR (
                          operation->>'type' = 'backup'
                          AND EXISTS (
                            SELECT 1
                            FROM backup_policies
                            WHERE backup_policies.schedule_id = schedules.id
                          )
                        )
                      )
                    ORDER BY {order_by}
                    LIMIT $1
                    OFFSET $2
                    "#,
                ))
                .bind(limit)
                .bind(offset)
                .bind(search_pattern(&query.q))
                .bind(require_backup_policy_metadata)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(schedule_from_postgres_row).collect()
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
            event_argv_template,
            selector_expression,
            target_client_ids,
            trigger_kind,
            cron_expr,
            timezone,
            event_expression,
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
                event_argv_template,
                selector_expression,
                target_client_ids,
                trigger_kind,
                cron_expr,
                timezone,
                event_expression,
                enabled,
                catch_up_policy,
                catch_up_limit,
                retry_delay_secs,
                max_failures,
                expected_definition_revision: None,
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
        match self {
            Self::Memory(memory) => {
                let _agent_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                crate::repository_key_lifecycle::require_visible_memory_clients(
                    memory,
                    &request.target_client_ids,
                    "schedule_fixed_targets_not_found",
                )
                .await?;
                let id = Uuid::new_v4();
                let now = unix_now();
                let created_at = now.to_string();
                let (next_run_at, event_armed_at) = match request.trigger_kind {
                    ScheduleTriggerKind::Cron => {
                        let cron_expr = request
                            .cron_expr
                            .as_deref()
                            .context("schedule cron expression is missing")?;
                        let next_runs = next_cron_runs(cron_expr, 5)?;
                        let next_run = next_runs
                            .first()
                            .cloned()
                            .context("schedule cron has no future occurrence")?;
                        (Some(next_run), None)
                    }
                    ScheduleTriggerKind::Event => (None, Some(created_at.clone())),
                };
                let operation_payload_hash = match request.trigger_kind {
                    ScheduleTriggerKind::Cron => valid_schedule_operation_payload_hash(
                        request
                            .operation
                            .as_ref()
                            .context("schedule cron operation is missing")?,
                    )?,
                    ScheduleTriggerKind::Event => event_schedule_operation_payload_hash(
                        request.event_argv_template.as_deref(),
                    )?,
                };
                let schedule = schedule_view_from_row(ScheduleRowParts {
                    id,
                    name: request.name,
                    enabled: request.enabled,
                    trigger_kind: request.trigger_kind,
                    definition_revision: 1,
                    operation: request.operation,
                    event_argv_template: request.event_argv_template,
                    operation_error: None,
                    operation_payload_hash,
                    selector_expression: request.selector_expression,
                    target_client_ids: request.target_client_ids,
                    cron_expr: request.cron_expr,
                    event_expression: request.event_expression,
                    event_armed_at,
                    timezone: request.timezone,
                    catch_up_policy: request.catch_up_policy,
                    catch_up_limit: request.catch_up_limit,
                    retry_delay_secs: request.retry_delay_secs,
                    max_failures: request.max_failures,
                    failure_count: 0,
                    last_error: None,
                    next_run_at,
                    last_run_at: None,
                    deferred_until: None,
                    deleted_at: None,
                    created_at: created_at.clone(),
                    updated_at: created_at,
                })?;
                let mut schedules = memory.schedules.write().await;
                schedules.push(schedule.clone());
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "schedule.created".to_string(),
                    target: format!("schedule:{}", schedule.id),
                    command_hash: None,
                    metadata: schedule_audit_metadata(&schedule, operator, serde_json::Value::Null),
                    created_at: now.to_string(),
                });
                Ok(schedule)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let schedule =
                    create_schedule_record_postgres_in_tx(&mut tx, &request, operator).await?;
                tx.commit().await?;
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
        expectation: Option<&ScheduleSnapshotExpectation>,
        operator: &AuthContext,
    ) -> Result<ScheduleView> {
        match self {
            Self::Memory(memory) => {
                let _agent_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                if !schedule_update_preserves_target_snapshot(&request, expectation) {
                    crate::repository_key_lifecycle::require_visible_memory_clients(
                        memory,
                        &request.target_client_ids,
                        "schedule_fixed_targets_not_found",
                    )
                    .await?;
                }
                let mut schedules = memory.schedules.write().await;
                let schedule = schedules
                    .iter_mut()
                    .find(|schedule| schedule.id == schedule_id && schedule.deleted_at.is_none())
                    .ok_or_else(|| anyhow::anyhow!("schedule_not_found:{schedule_id}"))?;
                ensure_schedule_snapshot(schedule, expectation)?;
                let schedule = apply_schedule_update_memory(schedule, &request)?;
                drop(schedules);
                record_memory_schedule_audit(memory, &schedule, operator, "schedule.updated").await;
                Ok(schedule)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let schedule = update_schedule_record_postgres_in_tx(
                    &mut tx,
                    schedule_id,
                    &request,
                    expectation,
                    operator,
                )
                .await?;
                tx.commit().await?;
                Ok(schedule)
            }
        }
    }

    pub(crate) async fn update_schedule_targets(
        &self,
        schedule_id: Uuid,
        target_client_ids: Vec<String>,
        expectation: Option<&ScheduleSnapshotExpectation>,
        operator: &AuthContext,
    ) -> Result<ScheduleView> {
        match self {
            Self::Memory(memory) => {
                let _agent_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                crate::repository_key_lifecycle::require_visible_memory_clients(
                    memory,
                    &target_client_ids,
                    "schedule_fixed_targets_not_found",
                )
                .await?;
                let now = unix_now().to_string();
                let mut schedules = memory.schedules.write().await;
                let schedule = schedules
                    .iter_mut()
                    .find(|schedule| schedule.id == schedule_id && schedule.deleted_at.is_none())
                    .ok_or_else(|| anyhow::anyhow!("schedule_not_found:{schedule_id}"))?;
                ensure_schedule_operation_valid(schedule)?;
                ensure_schedule_snapshot(schedule, expectation)?;
                schedule.target_client_ids = target_client_ids;
                schedule.definition_revision += 1;
                if schedule.trigger_kind == ScheduleTriggerKind::Event {
                    schedule.event_armed_at = Some(now.clone());
                }
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
                let mut tx = pool.begin().await?;
                crate::repository_key_lifecycle::require_visible_postgres_clients_in_tx(
                    &mut tx,
                    &target_client_ids,
                    "schedule_fixed_targets_not_found",
                )
                .await?;
                let schedule =
                    schedule_by_id_postgres_for_update_in_tx(&mut tx, schedule_id).await?;
                ensure_schedule_operation_valid(&schedule)?;
                ensure_schedule_snapshot(&schedule, expectation)?;
                let arm_boundary = lock_event_schedule_arm_boundary(&mut tx).await?;
                let result = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET
                        actor_id = $2,
                        target_client_ids = $3,
                        definition_revision = definition_revision + 1,
                        event_armed_at = CASE
                            WHEN trigger_kind = 'event' THEN clock_timestamp()
                            ELSE NULL
                        END,
                        armed_after_event_seq = CASE
                            WHEN trigger_kind = 'event' THEN $4
                            ELSE NULL
                        END,
                        updated_at = now()
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND definition_revision = $5
                    "#,
                )
                .bind(schedule_id)
                .bind(operator.operator.id)
                .bind(&target_client_ids)
                .bind(arm_boundary)
                .bind(expectation.map(|value| value.definition_revision))
                .execute(&mut *tx)
                .await?;
                anyhow::ensure!(
                    result.rows_affected() > 0,
                    "schedule_not_found:{schedule_id}"
                );
                let schedule = schedule_by_id_postgres_in_tx(&mut tx, schedule_id).await?;
                record_postgres_schedule_audit(
                    &mut tx,
                    &schedule,
                    operator,
                    "schedule.targets_updated",
                )
                .await?;
                tx.commit().await?;
                Ok(schedule)
            }
        }
    }

    pub(crate) async fn set_schedule_enabled(
        &self,
        schedule_id: Uuid,
        enabled: bool,
        expected_definition_revision: i64,
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
                ensure!(
                    schedule.definition_revision == expected_definition_revision,
                    "schedule_snapshot_stale"
                );
                if enabled {
                    ensure_schedule_operation_valid(schedule)?;
                }
                schedule.enabled = enabled;
                schedule.definition_revision += 1;
                if schedule.trigger_kind == ScheduleTriggerKind::Event {
                    schedule.event_armed_at = Some(now.clone());
                }
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
                let mut tx = pool.begin().await?;
                let schedule =
                    schedule_by_id_postgres_for_update_in_tx(&mut tx, schedule_id).await?;
                ensure!(
                    schedule.definition_revision == expected_definition_revision,
                    "schedule_snapshot_stale"
                );
                let arm_boundary = lock_event_schedule_arm_boundary(&mut tx).await?;
                if enabled {
                    ensure_schedule_operation_valid(&schedule)?;
                }
                let result = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET enabled = $2,
                        actor_id = $3,
                        definition_revision = definition_revision + 1,
                        event_armed_at = CASE
                            WHEN trigger_kind = 'event' THEN clock_timestamp()
                            ELSE NULL
                        END,
                        armed_after_event_seq = CASE
                            WHEN trigger_kind = 'event' THEN $5
                            ELSE NULL
                        END,
                        updated_at = now()
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND definition_revision = $4
                    "#,
                )
                .bind(schedule_id)
                .bind(enabled)
                .bind(operator.operator.id)
                .bind(expected_definition_revision)
                .bind(arm_boundary)
                .execute(&mut *tx)
                .await?;
                anyhow::ensure!(result.rows_affected() > 0, "schedule_snapshot_stale");
                let schedule = schedule_by_id_postgres_in_tx(&mut tx, schedule_id).await?;
                record_postgres_schedule_audit(
                    &mut tx,
                    &schedule,
                    operator,
                    if enabled {
                        "schedule.enabled"
                    } else {
                        "schedule.disabled"
                    },
                )
                .await?;
                tx.commit().await?;
                Ok(schedule)
            }
        }
    }

    pub(crate) async fn defer_schedule(
        &self,
        schedule_id: Uuid,
        deferred_until: &str,
        reason: Option<&str>,
        expected_definition_revision: i64,
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
                ensure!(
                    schedule.definition_revision == expected_definition_revision,
                    "schedule_snapshot_stale"
                );
                ensure_schedule_operation_valid(schedule)?;
                schedule.deferred_until = Some(deferred_until.to_string());
                schedule.definition_revision += 1;
                if schedule.trigger_kind == ScheduleTriggerKind::Event {
                    schedule.event_armed_at = Some(now.clone());
                }
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
                let mut tx = pool.begin().await?;
                let schedule =
                    schedule_by_id_postgres_for_update_in_tx(&mut tx, schedule_id).await?;
                ensure!(
                    schedule.definition_revision == expected_definition_revision,
                    "schedule_snapshot_stale"
                );
                ensure_schedule_operation_valid(&schedule)?;
                let arm_boundary = lock_event_schedule_arm_boundary(&mut tx).await?;
                let result = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET deferred_until = to_timestamp($2),
                        actor_id = $3,
                        definition_revision = definition_revision + 1,
                        event_armed_at = CASE
                            WHEN trigger_kind = 'event' THEN clock_timestamp()
                            ELSE NULL
                        END,
                        armed_after_event_seq = CASE
                            WHEN trigger_kind = 'event' THEN $5
                            ELSE NULL
                        END,
                        updated_at = now()
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND definition_revision = $4
                    "#,
                )
                .bind(schedule_id)
                .bind(next_run_timestamp(deferred_until)? as f64)
                .bind(operator.operator.id)
                .bind(expected_definition_revision)
                .bind(arm_boundary)
                .execute(&mut *tx)
                .await?;
                anyhow::ensure!(result.rows_affected() > 0, "schedule_snapshot_stale");
                let schedule = schedule_by_id_postgres_in_tx(&mut tx, schedule_id).await?;
                record_postgres_schedule_audit_with_extra(
                    &mut tx,
                    &schedule,
                    operator,
                    "schedule.deferred",
                    serde_json::json!({ "reason": reason }),
                )
                .await?;
                tx.commit().await?;
                Ok(schedule)
            }
        }
    }

    pub(crate) async fn soft_delete_schedule(
        &self,
        schedule_id: Uuid,
        expected_definition_revision: i64,
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
                ensure!(
                    schedule.definition_revision == expected_definition_revision,
                    "schedule_snapshot_stale"
                );
                schedule.enabled = false;
                schedule.definition_revision += 1;
                schedule.deleted_at = Some(now.clone());
                schedule.updated_at = now;
                let schedule = schedule.clone();
                drop(schedules);
                record_memory_schedule_audit(memory, &schedule, operator, "schedule.deleted").await;
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let sql = schedule_select_sql("WHERE id = $1 AND deleted_at IS NULL FOR UPDATE");
                let row = sqlx::query(&sql)
                    .bind(schedule_id)
                    .fetch_one(&mut *tx)
                    .await?;
                let schedule = schedule_from_postgres_row(row)?;
                ensure!(
                    schedule.definition_revision == expected_definition_revision,
                    "schedule_snapshot_stale"
                );
                let _arm_boundary = lock_event_schedule_arm_boundary(&mut tx).await?;
                let result = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET deleted_at = now(),
                        deleted_by = $2,
                        actor_id = $2,
                        enabled = FALSE,
                        definition_revision = definition_revision + 1,
                        updated_at = now()
                    WHERE id = $1
                      AND deleted_at IS NULL
                      AND definition_revision = $3
                    RETURNING
                        deleted_at::text AS deleted_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(schedule_id)
                .bind(operator.operator.id)
                .bind(expected_definition_revision)
                .fetch_optional(&mut *tx)
                .await?;
                let result = result.ok_or_else(|| anyhow::anyhow!("schedule_snapshot_stale"))?;
                let mut schedule = schedule;
                schedule.enabled = false;
                schedule.definition_revision += 1;
                schedule.deleted_at = result.try_get("deleted_at")?;
                schedule.updated_at = result.try_get("updated_at")?;
                record_postgres_schedule_audit(&mut tx, &schedule, operator, "schedule.deleted")
                    .await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }
}

pub(crate) struct ScheduleCreateInput {
    pub(crate) name: String,
    pub(crate) operation: Option<vpsman_common::JobCommand>,
    pub(crate) event_argv_template: Option<Vec<String>>,
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) trigger_kind: ScheduleTriggerKind,
    pub(crate) cron_expr: Option<String>,
    pub(crate) timezone: Option<String>,
    pub(crate) event_expression: Option<String>,
    pub(crate) enabled: bool,
    pub(crate) catch_up_policy: Option<String>,
    pub(crate) catch_up_limit: Option<i32>,
    pub(crate) retry_delay_secs: Option<i64>,
    pub(crate) max_failures: i32,
    pub(crate) expected_definition_revision: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct ScheduleSnapshotExpectation {
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) definition_revision: i64,
}

struct ScheduleRowParts {
    id: Uuid,
    name: String,
    enabled: bool,
    trigger_kind: ScheduleTriggerKind,
    definition_revision: i64,
    operation: Option<JobCommand>,
    event_argv_template: Option<Vec<String>>,
    operation_error: Option<String>,
    operation_payload_hash: String,
    selector_expression: String,
    target_client_ids: Vec<String>,
    cron_expr: Option<String>,
    event_expression: Option<String>,
    event_armed_at: Option<String>,
    timezone: Option<String>,
    catch_up_policy: Option<String>,
    catch_up_limit: Option<i32>,
    retry_delay_secs: Option<i64>,
    max_failures: i32,
    failure_count: i32,
    last_error: Option<String>,
    next_run_at: Option<String>,
    last_run_at: Option<String>,
    deferred_until: Option<String>,
    deleted_at: Option<String>,
    created_at: String,
    updated_at: String,
}

fn schedule_view_from_row(parts: ScheduleRowParts) -> Result<ScheduleView> {
    let command_type = if parts.trigger_kind == ScheduleTriggerKind::Event {
        "shell".to_string()
    } else {
        parts
            .operation
            .as_ref()
            .map(job_command_type_label)
            .unwrap_or(INVALID_SCHEDULE_COMMAND_TYPE)
            .to_string()
    };
    let (next_runs, cadence_error) = match (parts.trigger_kind, parts.cron_expr.as_deref()) {
        (ScheduleTriggerKind::Cron, Some(cron_expr)) => stored_cron_preview(cron_expr, 5),
        (ScheduleTriggerKind::Cron, None) => (
            Vec::new(),
            Some("schedule cron expression is missing".to_string()),
        ),
        (ScheduleTriggerKind::Event, _) => (Vec::new(), None),
    };
    Ok(ScheduleView {
        id: parts.id,
        name: parts.name,
        enabled: parts.enabled,
        trigger_kind: parts.trigger_kind,
        definition_revision: parts.definition_revision,
        command_type,
        operation: parts.operation,
        event_argv_template: parts.event_argv_template,
        operation_error: parts.operation_error,
        operation_payload_hash: parts.operation_payload_hash,
        selector_expression: parts.selector_expression,
        target_client_ids: parts.target_client_ids,
        next_runs,
        cron_expr: parts.cron_expr,
        event_expression: parts.event_expression,
        event_armed_at: parts.event_armed_at,
        timezone: parts.timezone,
        catch_up_policy: parts.catch_up_policy,
        catch_up_limit: parts.catch_up_limit,
        retry_delay_secs: parts.retry_delay_secs,
        max_failures: parts.max_failures,
        failure_count: parts.failure_count,
        last_error: parts.last_error,
        next_run_at: parts.next_run_at,
        cadence_error,
        last_run_at: parts.last_run_at,
        deferred_until: parts.deferred_until,
        deleted_at: parts.deleted_at,
        created_at: parts.created_at,
        updated_at: parts.updated_at,
    })
}

fn schedule_select_sql(where_clause: &str) -> String {
    format!(
        r#"
        SELECT
            id,
            name,
            enabled,
            trigger_kind,
            definition_revision,
            operation,
            event_argv_template,
            selector_expression,
            target_client_ids,
            cron_expr,
            event_expression,
            event_armed_at::text AS event_armed_at,
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
    let trigger_kind = match row.try_get::<String, _>("trigger_kind")?.as_str() {
        "cron" => ScheduleTriggerKind::Cron,
        "event" => ScheduleTriggerKind::Event,
        other => anyhow::bail!("invalid_schedule_trigger_kind:{other}"),
    };
    let event_argv_template = row
        .try_get::<Option<SqlJson<Vec<String>>>, _>("event_argv_template")?
        .map(|value| value.0);
    let operation = row
        .try_get::<Option<SqlJson<Value>>, _>("operation")?
        .map(|value| value.0);
    let (operation, operation_error, operation_payload_hash) =
        decode_stored_schedule_operation(operation, trigger_kind, event_argv_template.as_deref());
    schedule_view_from_row(ScheduleRowParts {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        enabled: row.try_get("enabled")?,
        trigger_kind,
        definition_revision: row.try_get("definition_revision")?,
        operation,
        event_argv_template,
        operation_error,
        operation_payload_hash,
        selector_expression: row.try_get("selector_expression")?,
        target_client_ids: row.try_get("target_client_ids")?,
        cron_expr: row.try_get("cron_expr")?,
        event_expression: row.try_get("event_expression")?,
        event_armed_at: row.try_get("event_armed_at")?,
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
    })
}

pub(crate) async fn create_schedule_record_postgres_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    request: &ScheduleCreateInput,
    operator: &AuthContext,
) -> Result<ScheduleView> {
    crate::repository_key_lifecycle::require_visible_postgres_clients_in_tx(
        tx,
        &request.target_client_ids,
        "schedule_fixed_targets_not_found",
    )
    .await?;
    let id = Uuid::new_v4();
    let next_run_unix = match request.trigger_kind {
        ScheduleTriggerKind::Cron => {
            let cron_expr = request
                .cron_expr
                .as_deref()
                .context("schedule cron expression is missing")?;
            let next_run = next_cron_runs(cron_expr, 1)?
                .into_iter()
                .next()
                .context("schedule cron has no future occurrence")?;
            Some(next_run_timestamp(&next_run)? as f64)
        }
        ScheduleTriggerKind::Event => None,
    };
    let armed_after_event_seq = if request.trigger_kind == ScheduleTriggerKind::Event {
        Some(lock_event_schedule_arm_boundary(tx).await?)
    } else {
        None
    };
    let row = sqlx::query(
        r#"
        INSERT INTO schedules (
            id,
            actor_id,
            name,
            enabled,
            trigger_kind,
            operation,
            event_argv_template,
            selector_expression,
            target_client_ids,
            cron_expr,
            timezone,
            event_expression,
            catch_up_policy,
            catch_up_limit,
            retry_delay_secs,
            max_failures,
            next_run_at,
            event_armed_at,
            armed_after_event_seq
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
            $11, $12, $13, $14, $15, $16, to_timestamp($17),
            CASE WHEN $5 = 'event' THEN clock_timestamp() ELSE NULL END,
            $18
        )
        RETURNING
            id,
            name,
            enabled,
            trigger_kind,
            definition_revision,
            operation,
            event_argv_template,
            selector_expression,
            target_client_ids,
            cron_expr,
            event_expression,
            event_armed_at::text AS event_armed_at,
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
    .bind(schedule_trigger_kind_storage(request.trigger_kind))
    .bind(request.operation.as_ref().map(SqlJson))
    .bind(request.event_argv_template.as_ref().map(SqlJson))
    .bind(request.selector_expression.trim())
    .bind(&request.target_client_ids)
    .bind(request.cron_expr.as_deref())
    .bind(request.timezone.as_deref())
    .bind(request.event_expression.as_deref())
    .bind(request.catch_up_policy.as_deref())
    .bind(request.catch_up_limit)
    .bind(request.retry_delay_secs)
    .bind(request.max_failures)
    .bind(next_run_unix)
    .bind(armed_after_event_seq)
    .fetch_one(&mut **tx)
    .await?;
    let schedule = schedule_from_postgres_row(row)?;
    record_postgres_schedule_audit(tx, &schedule, operator, "schedule.created").await?;
    Ok(schedule)
}

pub(crate) async fn update_schedule_record_postgres_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: Uuid,
    request: &ScheduleCreateInput,
    expectation: Option<&ScheduleSnapshotExpectation>,
    operator: &AuthContext,
) -> Result<ScheduleView> {
    if !schedule_update_preserves_target_snapshot(request, expectation) {
        crate::repository_key_lifecycle::require_visible_postgres_clients_in_tx(
            tx,
            &request.target_client_ids,
            "schedule_fixed_targets_not_found",
        )
        .await?;
    }
    let next_run_unix = match request.trigger_kind {
        ScheduleTriggerKind::Cron => {
            let cron_expr = request
                .cron_expr
                .as_deref()
                .context("schedule cron expression is missing")?;
            let next_run = next_cron_runs(cron_expr, 1)?
                .into_iter()
                .next()
                .context("schedule cron has no future occurrence")?;
            Some(next_run_timestamp(&next_run)? as f64)
        }
        ScheduleTriggerKind::Event => None,
    };
    let current = schedule_by_id_postgres_for_update_in_tx(tx, schedule_id).await?;
    ensure_schedule_snapshot(&current, expectation)?;
    let arm_boundary = lock_event_schedule_arm_boundary(tx).await?;
    let armed_after_event_seq =
        (request.trigger_kind == ScheduleTriggerKind::Event).then_some(arm_boundary);
    let result = sqlx::query(
        r#"
        UPDATE schedules
        SET
            actor_id = $2,
            name = $3,
            enabled = $4,
            trigger_kind = $5,
            operation = $6,
            event_argv_template = $7,
            selector_expression = $8,
            target_client_ids = $9,
            cron_expr = $10,
            timezone = $11,
            event_expression = $12,
            catch_up_policy = $13,
            catch_up_limit = $14,
            retry_delay_secs = $15,
            max_failures = $16,
            next_run_at = to_timestamp($17),
            event_armed_at = CASE WHEN $5 = 'event' THEN clock_timestamp() ELSE NULL END,
            armed_after_event_seq = $18,
            definition_revision = definition_revision + 1,
            failure_count = 0,
            last_error = NULL,
            updated_at = now()
        WHERE id = $1
          AND deleted_at IS NULL
          AND definition_revision = $19
        "#,
    )
    .bind(schedule_id)
    .bind(operator.operator.id)
    .bind(&request.name)
    .bind(request.enabled)
    .bind(schedule_trigger_kind_storage(request.trigger_kind))
    .bind(request.operation.as_ref().map(SqlJson))
    .bind(request.event_argv_template.as_ref().map(SqlJson))
    .bind(request.selector_expression.trim())
    .bind(&request.target_client_ids)
    .bind(request.cron_expr.as_deref())
    .bind(request.timezone.as_deref())
    .bind(request.event_expression.as_deref())
    .bind(request.catch_up_policy.as_deref())
    .bind(request.catch_up_limit)
    .bind(request.retry_delay_secs)
    .bind(request.max_failures)
    .bind(next_run_unix)
    .bind(armed_after_event_seq)
    .bind(request.expected_definition_revision)
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() > 0,
        "schedule_not_found:{schedule_id}"
    );
    let schedule = schedule_by_id_postgres_in_tx(tx, schedule_id).await?;
    record_postgres_schedule_audit(tx, &schedule, operator, "schedule.updated").await?;
    Ok(schedule)
}

pub(crate) async fn schedule_by_id_postgres_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: Uuid,
) -> Result<ScheduleView> {
    let sql = schedule_select_sql("WHERE id = $1 AND deleted_at IS NULL");
    let row = sqlx::query(&sql)
        .bind(schedule_id)
        .fetch_one(&mut **tx)
        .await?;
    schedule_from_postgres_row(row)
}

async fn schedule_by_id_postgres_for_update_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: Uuid,
) -> Result<ScheduleView> {
    let sql = schedule_select_sql("WHERE id = $1 AND deleted_at IS NULL FOR UPDATE");
    let row = sqlx::query(&sql)
        .bind(schedule_id)
        .fetch_one(&mut **tx)
        .await?;
    schedule_from_postgres_row(row)
}

pub(crate) async fn backup_policy_schedule_by_id_postgres_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: Uuid,
) -> Result<Option<ScheduleView>> {
    let sql = schedule_select_sql(
        r#"
        WHERE id = $1
          AND deleted_at IS NULL
          AND operation->>'type' = 'backup'
          AND EXISTS (
            SELECT 1
            FROM backup_policies
            WHERE backup_policies.schedule_id = schedules.id
          )
        FOR UPDATE
        "#,
    );
    let row = sqlx::query(&sql)
        .bind(schedule_id)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(schedule_from_postgres_row).transpose()
}

pub(crate) async fn backup_policy_schedule_by_id_postgres(
    pool: &sqlx::PgPool,
    schedule_id: Uuid,
) -> Result<Option<ScheduleView>> {
    let sql = schedule_select_sql(
        r#"
        WHERE id = $1
          AND deleted_at IS NULL
          AND operation->>'type' = 'backup'
          AND EXISTS (
            SELECT 1
            FROM backup_policies
            WHERE backup_policies.schedule_id = schedules.id
          )
        "#,
    );
    let row = sqlx::query(&sql)
        .bind(schedule_id)
        .fetch_optional(pool)
        .await?;
    row.map(schedule_from_postgres_row).transpose()
}

pub(crate) fn apply_schedule_update_memory(
    schedule: &mut ScheduleView,
    request: &ScheduleCreateInput,
) -> Result<ScheduleView> {
    let (next_runs, next_run_at, event_armed_at) = match request.trigger_kind {
        ScheduleTriggerKind::Cron => {
            let cron_expr = request
                .cron_expr
                .as_deref()
                .context("schedule cron expression is missing")?;
            let next_runs = next_cron_runs(cron_expr, 5)?;
            let next_run = next_runs
                .first()
                .cloned()
                .context("schedule cron has no future occurrence")?;
            (next_runs, Some(next_run), None)
        }
        ScheduleTriggerKind::Event => (Vec::new(), None, Some(unix_now().to_string())),
    };
    schedule.name = request.name.clone();
    schedule.enabled = request.enabled;
    schedule.trigger_kind = request.trigger_kind;
    schedule.definition_revision += 1;
    schedule.command_type = request
        .operation
        .as_ref()
        .map(job_command_type_label)
        .unwrap_or("shell")
        .to_string();
    schedule.operation_payload_hash = match request.trigger_kind {
        ScheduleTriggerKind::Cron => valid_schedule_operation_payload_hash(
            request
                .operation
                .as_ref()
                .context("schedule cron operation is missing")?,
        )?,
        ScheduleTriggerKind::Event => {
            event_schedule_operation_payload_hash(request.event_argv_template.as_deref())?
        }
    };
    schedule.operation = request.operation.clone();
    schedule.event_argv_template = request.event_argv_template.clone();
    schedule.operation_error = None;
    schedule.selector_expression = request.selector_expression.clone();
    schedule.target_client_ids = request.target_client_ids.clone();
    schedule.cron_expr = request.cron_expr.clone();
    schedule.event_expression = request.event_expression.clone();
    schedule.event_armed_at = event_armed_at;
    schedule.timezone = request.timezone.clone();
    schedule.next_runs = next_runs;
    schedule.cadence_error = None;
    schedule.catch_up_policy = request.catch_up_policy.clone();
    schedule.catch_up_limit = request.catch_up_limit;
    schedule.retry_delay_secs = request.retry_delay_secs;
    schedule.max_failures = request.max_failures;
    schedule.failure_count = 0;
    schedule.last_error = None;
    schedule.next_run_at = next_run_at;
    schedule.updated_at = unix_now().to_string();
    Ok(schedule.clone())
}

pub(crate) fn ensure_schedule_snapshot(
    schedule: &ScheduleView,
    expectation: Option<&ScheduleSnapshotExpectation>,
) -> Result<()> {
    let Some(expectation) = expectation else {
        return Ok(());
    };
    let mut expected_targets = expectation.target_client_ids.clone();
    expected_targets.sort();
    expected_targets.dedup();
    let mut stored_targets = schedule.target_client_ids.clone();
    stored_targets.sort();
    stored_targets.dedup();
    anyhow::ensure!(
        schedule.selector_expression.trim() == expectation.selector_expression.trim()
            && stored_targets == expected_targets
            && schedule.definition_revision == expectation.definition_revision,
        "schedule_snapshot_stale"
    );
    Ok(())
}

pub(crate) fn schedule_update_preserves_target_snapshot(
    request: &ScheduleCreateInput,
    expectation: Option<&ScheduleSnapshotExpectation>,
) -> bool {
    let Some(expectation) = expectation else {
        return false;
    };
    let mut requested_targets = request.target_client_ids.clone();
    requested_targets.sort();
    requested_targets.dedup();
    let mut expected_targets = expectation.target_client_ids.clone();
    expected_targets.sort();
    expected_targets.dedup();
    request.selector_expression.trim() == expectation.selector_expression.trim()
        && requested_targets == expected_targets
}

pub(crate) async fn record_memory_schedule_audit(
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
    tx: &mut Transaction<'_, Postgres>,
    schedule: &ScheduleView,
    operator: &AuthContext,
    action: &str,
) -> Result<()> {
    record_postgres_schedule_audit_with_extra(
        tx,
        schedule,
        operator,
        action,
        serde_json::Value::Null,
    )
    .await
}

async fn record_postgres_schedule_audit_with_extra(
    tx: &mut Transaction<'_, Postgres>,
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
    .execute(&mut **tx)
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
        "operation_error": &schedule.operation_error,
        "operation_payload_hash": &schedule.operation_payload_hash,
        "selector_expression": &schedule.selector_expression,
        "target_client_ids": &schedule.target_client_ids,
        "target_count": schedule.target_client_ids.len(),
        "cron_expr": &schedule.cron_expr,
        "timezone": &schedule.timezone,
        "next_runs": &schedule.next_runs,
        "cadence_error": &schedule.cadence_error,
        "catch_up_policy": &schedule.catch_up_policy,
        "catch_up_limit": schedule.catch_up_limit,
        "retry_delay_secs": schedule.retry_delay_secs,
        "max_failures": schedule.max_failures,
        "enabled": schedule.enabled,
        "deferred_until": schedule.deferred_until,
        "deleted_at": schedule.deleted_at,
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "schedule-controller",
    });
    if !extra.is_null() {
        metadata["extra"] = extra;
    }
    metadata
}

pub(crate) fn next_cron_runs(cron_expr: &str, count: usize) -> Result<Vec<String>> {
    let runs = cron_runs_after_now(cron_expr, count)?;
    ensure!(
        count == 0 || !runs.is_empty(),
        "schedule cron has no future occurrence"
    );
    Ok(runs)
}

fn cron_runs_after_now(cron_expr: &str, count: usize) -> Result<Vec<String>> {
    let cron = Cron::from_str(cron_expr)?;
    Ok(cron
        .iter_after(Utc::now())
        .take(count)
        .map(|run| run.to_rfc3339())
        .collect())
}

fn stored_cron_preview(cron_expr: &str, count: usize) -> (Vec<String>, Option<String>) {
    match cron_runs_after_now(cron_expr, count) {
        Ok(runs) if count > 0 && runs.is_empty() => {
            (runs, Some("schedule_cron_no_future_occurrence".to_string()))
        }
        Ok(runs) => (runs, None),
        Err(_) => (Vec::new(), Some("schedule_cron_invalid".to_string())),
    }
}

fn next_run_timestamp(value: &str) -> Result<i64> {
    Ok(DateTime::parse_from_rfc3339(value)?.timestamp())
}
