use std::collections::{BTreeSet, HashSet};

use anyhow::{Context, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{AuditLogView, AuthContext},
    model_alert_states::{
        BulkFleetAlertStateItem, BulkUpdateFleetAlertStatesRequest,
        BulkUpdateFleetAlertStatesResponse, FleetAlertStateView, UpdateFleetAlertStateRequest,
    },
    repository::Repository,
    unix_now,
};

const ALERT_STATE_OPEN: &str = "open";
const ALERT_STATE_ACKNOWLEDGED: &str = "acknowledged";
const ALERT_STATE_MUTED: &str = "muted";
const ALERT_STATE_ESCALATED: &str = "escalated";
const ACTION_ACKNOWLEDGE: &str = "acknowledge";
const ACTION_MUTE: &str = "mute";
const ACTION_ESCALATE: &str = "escalate";
const ACTION_CLEAR: &str = "clear";
const MAX_ALERT_ID_BYTES: usize = 192;
const MAX_ALERT_REASON_BYTES: usize = 1024;
const DEFAULT_MUTE_SECS: i64 = 3600;
const MAX_MUTE_SECS: i64 = 90 * 24 * 60 * 60;

impl Repository {
    pub(crate) async fn list_fleet_alert_states(
        &self,
        limit: i64,
        state: Option<&str>,
    ) -> Result<Vec<FleetAlertStateView>> {
        let state = normalize_optional_state(state)?;
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .fleet_alert_states
                    .read()
                    .await
                    .iter()
                    .filter(|row| state.as_deref().is_none_or(|state| row.state == state))
                    .cloned()
                    .collect::<Vec<_>>();
                sort_alert_states(&mut rows);
                rows.truncate(limit.clamp(1, 1000) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        alert_id,
                        state,
                        muted_until_unix,
                        escalation_level,
                        revision,
                        reason,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM fleet_alert_states
                    WHERE ($2::text IS NULL OR state = $2)
                    ORDER BY updated_at DESC, alert_id ASC
                    LIMIT $1
                    "#,
                )
                .bind(limit.clamp(1, 1000))
                .bind(state.as_deref())
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(alert_state_from_row).collect()
            }
        }
    }

    pub(crate) async fn list_fleet_alert_states_for_alert_ids(
        &self,
        alert_ids: &[String],
    ) -> Result<Vec<FleetAlertStateView>> {
        if alert_ids.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let alert_ids = alert_ids.iter().map(String::as_str).collect::<HashSet<_>>();
                let mut rows = memory
                    .fleet_alert_states
                    .read()
                    .await
                    .iter()
                    .filter(|row| alert_ids.contains(row.alert_id.as_str()))
                    .cloned()
                    .collect::<Vec<_>>();
                sort_alert_states(&mut rows);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        alert_id,
                        state,
                        muted_until_unix,
                        escalation_level,
                        revision,
                        reason,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM fleet_alert_states
                    WHERE alert_id = ANY($1::text[])
                    ORDER BY updated_at DESC, alert_id ASC
                    "#,
                )
                .bind(alert_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(alert_state_from_row).collect()
            }
        }
    }

    pub(crate) async fn update_fleet_alert_state(
        &self,
        request: &UpdateFleetAlertStateRequest,
        operator: &AuthContext,
    ) -> Result<FleetAlertStateView> {
        anyhow::ensure!(request.confirmed, "fleet_alert_state_confirmation_required");
        let item = BulkFleetAlertStateItem {
            alert_id: request.alert_id.clone(),
            expected_revision: request.expected_revision.unwrap_or(0),
        };
        let batch_id = Uuid::new_v4();
        let mut states = self
            .mutate_fleet_alert_states(
                request.action.trim(),
                std::slice::from_ref(&item),
                request.muted_for_secs,
                request.reason.as_deref(),
                request.expected_revision.is_some(),
                batch_id,
                operator,
            )
            .await?;
        states
            .pop()
            .context("fleet alert state mutation returned no row")
    }

    pub(crate) async fn bulk_update_fleet_alert_states(
        &self,
        request: &BulkUpdateFleetAlertStatesRequest,
        operator: &AuthContext,
    ) -> Result<BulkUpdateFleetAlertStatesResponse> {
        anyhow::ensure!(request.confirmed, "fleet_alert_state_confirmation_required");
        let batch_id = Uuid::new_v4();
        let states = self
            .mutate_fleet_alert_states(
                request.action.trim(),
                &request.items,
                request.muted_for_secs,
                request.reason.as_deref(),
                true,
                batch_id,
                operator,
            )
            .await?;
        Ok(BulkUpdateFleetAlertStatesResponse { batch_id, states })
    }

    #[allow(clippy::too_many_arguments)]
    async fn mutate_fleet_alert_states(
        &self,
        action: &str,
        items: &[BulkFleetAlertStateItem],
        muted_for_secs: Option<i64>,
        reason: Option<&str>,
        enforce_expected_revision: bool,
        batch_id: Uuid,
        operator: &AuthContext,
    ) -> Result<Vec<FleetAlertStateView>> {
        let items = normalize_mutation_items(items)?;
        validate_alert_state_action(action, muted_for_secs)?;
        validate_alert_reason(reason)?;
        let now_unix = unix_now();
        let now = now_unix.to_string();
        match self {
            Self::Memory(memory) => {
                let mut states = memory.fleet_alert_states.write().await;
                validate_expected_revisions(&states, &items, enforce_expected_revision)?;
                let mut next_states = states.clone();
                let mut changed = Vec::with_capacity(items.len());
                for item in &items {
                    let current = next_states
                        .iter()
                        .find(|state| state.alert_id == item.alert_id)
                        .cloned();
                    let next = transition_alert_state(
                        current.as_ref(),
                        &item.alert_id,
                        action,
                        muted_for_secs,
                        reason,
                        now_unix,
                        &now,
                        operator,
                    )?;
                    if let Some(stored) = next_states
                        .iter_mut()
                        .find(|state| state.alert_id == item.alert_id)
                    {
                        *stored = next.clone();
                    } else {
                        next_states.push(next.clone());
                    }
                    changed.push(next);
                }
                let mut audits = memory.audits.write().await;
                *states = next_states;
                audits.extend(changed.iter().map(|state| {
                    alert_state_audit(state, operator, now.clone(), batch_id, items.len(), action)
                }));
                Ok(changed)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let alert_ids = items
                    .iter()
                    .map(|item| item.alert_id.clone())
                    .collect::<Vec<_>>();
                sqlx::query(
                    r#"
                    INSERT INTO fleet_alert_states (
                        alert_id, state, muted_until_unix, escalation_level,
                        revision, reason, actor_id
                    )
                    SELECT alert_id, 'open', NULL, 0, 0, NULL, NULL
                    FROM unnest($1::TEXT[]) AS input(alert_id)
                    ORDER BY alert_id
                    ON CONFLICT (alert_id) DO NOTHING
                    "#,
                )
                .bind(&alert_ids)
                .execute(&mut *tx)
                .await?;
                let rows = sqlx::query(
                    r#"
                    SELECT
                        alert_id,
                        state,
                        muted_until_unix,
                        escalation_level,
                        revision,
                        reason,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM fleet_alert_states
                    WHERE alert_id = ANY($1::TEXT[])
                    ORDER BY alert_id
                    FOR UPDATE
                    "#,
                )
                .bind(&alert_ids)
                .fetch_all(&mut *tx)
                .await?;
                let current = rows
                    .into_iter()
                    .map(alert_state_from_row)
                    .collect::<Result<Vec<_>>>()?;
                anyhow::ensure!(
                    current.len() == items.len(),
                    "fleet_alert_state_lock_incomplete"
                );
                validate_expected_revisions(&current, &items, enforce_expected_revision)?;
                let mut candidates = Vec::with_capacity(items.len());
                for item in &items {
                    let stored = current
                        .iter()
                        .find(|state| state.alert_id == item.alert_id)
                        .context("fleet_alert_state_lock_incomplete")?;
                    candidates.push(transition_alert_state(
                        Some(stored),
                        &item.alert_id,
                        action,
                        muted_for_secs,
                        reason,
                        now_unix,
                        &now,
                        operator,
                    )?);
                }
                let mutations = serde_json::Value::Array(
                    candidates
                        .iter()
                        .map(|candidate| {
                            json!({
                                "alert_id": candidate.alert_id,
                                "state": candidate.state,
                                "muted_until_unix": candidate.muted_until_unix,
                                "escalation_level": candidate.escalation_level,
                                "reason": candidate.reason,
                                "expected_revision": candidate.revision - 1,
                            })
                        })
                        .collect(),
                );
                let rows = sqlx::query(
                    r#"
                    UPDATE fleet_alert_states AS stored
                    SET state = mutation.state,
                        muted_until_unix = mutation.muted_until_unix,
                        escalation_level = mutation.escalation_level,
                        revision = stored.revision + 1,
                        reason = mutation.reason,
                        actor_id = $2,
                        updated_at = now()
                    FROM jsonb_to_recordset($1::JSONB) AS mutation(
                        alert_id TEXT,
                        state TEXT,
                        muted_until_unix BIGINT,
                        escalation_level INTEGER,
                        reason TEXT,
                        expected_revision BIGINT
                    )
                    WHERE stored.alert_id = mutation.alert_id
                      AND stored.revision = mutation.expected_revision
                    RETURNING
                        stored.alert_id,
                        stored.state,
                        stored.muted_until_unix,
                        stored.escalation_level,
                        stored.revision,
                        stored.reason,
                        stored.actor_id,
                        stored.created_at::text AS created_at,
                        stored.updated_at::text AS updated_at
                    "#,
                )
                .bind(mutations)
                .bind(operator.operator.id)
                .fetch_all(&mut *tx)
                .await?;
                anyhow::ensure!(
                    rows.len() == items.len(),
                    "fleet_alert_state_snapshot_stale"
                );
                let mut changed = rows
                    .into_iter()
                    .map(alert_state_from_row)
                    .collect::<Result<Vec<_>>>()?;
                changed.sort_by(|left, right| left.alert_id.cmp(&right.alert_id));
                let audit_rows = serde_json::Value::Array(
                    changed
                        .iter()
                        .map(|state| {
                            json!({
                                "id": Uuid::new_v4(),
                                "target": format!("fleet_alert:{}", state.alert_id),
                                "metadata": alert_state_metadata(
                                    state,
                                    operator,
                                    batch_id,
                                    items.len(),
                                    action,
                                ),
                            })
                        })
                        .collect(),
                );
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    SELECT audit.id, $2, 'fleet.alert_state_updated',
                        audit.target, NULL, audit.metadata
                    FROM jsonb_to_recordset($1::JSONB) AS audit(
                        id UUID,
                        target TEXT,
                        metadata JSONB
                    )
                    "#,
                )
                .bind(audit_rows)
                .bind(operator.operator.id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(changed)
            }
        }
    }
}

fn transition_alert_state(
    current: Option<&FleetAlertStateView>,
    alert_id: &str,
    action: &str,
    muted_for_secs: Option<i64>,
    reason: Option<&str>,
    now_unix: u64,
    now: &str,
    operator: &AuthContext,
) -> Result<FleetAlertStateView> {
    let current_escalation = current.map_or(0, |state| state.escalation_level);
    let (state, muted_until_unix, escalation_level) = match action {
        ACTION_ACKNOWLEDGE => (
            ALERT_STATE_ACKNOWLEDGED.to_string(),
            None,
            current_escalation,
        ),
        ACTION_MUTE => {
            let seconds = muted_for_secs.unwrap_or(DEFAULT_MUTE_SECS);
            (
                ALERT_STATE_MUTED.to_string(),
                Some((now_unix as i64).saturating_add(seconds)),
                current_escalation,
            )
        }
        ACTION_ESCALATE => (
            ALERT_STATE_ESCALATED.to_string(),
            None,
            current_escalation
                .checked_add(1)
                .context("fleet_alert_escalation_level_overflow")?,
        ),
        ACTION_CLEAR => (ALERT_STATE_OPEN.to_string(), None, 0),
        _ => anyhow::bail!("fleet_alert_state_action_invalid"),
    };
    let revision = current
        .map_or(0, |state| state.revision)
        .checked_add(1)
        .context("fleet_alert_state_revision_overflow")?;
    Ok(FleetAlertStateView {
        alert_id: alert_id.to_string(),
        state,
        muted_until_unix,
        escalation_level,
        revision,
        reason: reason
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(ToOwned::to_owned),
        actor_id: Some(operator.operator.id),
        created_at: current
            .map(|state| state.created_at.clone())
            .unwrap_or_else(|| now.to_string()),
        updated_at: now.to_string(),
    })
}

fn normalize_mutation_items(
    items: &[BulkFleetAlertStateItem],
) -> Result<Vec<BulkFleetAlertStateItem>> {
    anyhow::ensure!(!items.is_empty(), "fleet_alert_state_items_invalid");
    anyhow::ensure!(items.len() <= 1_000, "fleet_alert_state_items_invalid");
    let mut normalized = Vec::with_capacity(items.len());
    let mut alert_ids = BTreeSet::new();
    for item in items {
        validate_alert_id(&item.alert_id)?;
        anyhow::ensure!(
            item.expected_revision >= 0,
            "fleet_alert_state_expected_revision_invalid"
        );
        let alert_id = item.alert_id.trim().to_string();
        anyhow::ensure!(
            alert_ids.insert(alert_id.clone()),
            "fleet_alert_state_duplicate_item"
        );
        normalized.push(BulkFleetAlertStateItem {
            alert_id,
            expected_revision: item.expected_revision,
        });
    }
    normalized.sort_by(|left, right| left.alert_id.cmp(&right.alert_id));
    Ok(normalized)
}

fn validate_expected_revisions(
    states: &[FleetAlertStateView],
    items: &[BulkFleetAlertStateItem],
    enforce: bool,
) -> Result<()> {
    if !enforce {
        return Ok(());
    }
    for item in items {
        let current_revision = states
            .iter()
            .find(|state| state.alert_id == item.alert_id)
            .map_or(0, |state| state.revision);
        anyhow::ensure!(
            current_revision == item.expected_revision,
            "fleet_alert_state_snapshot_stale"
        );
    }
    Ok(())
}

fn validate_alert_state_action(action: &str, muted_for_secs: Option<i64>) -> Result<()> {
    match action {
        ACTION_MUTE => {
            let seconds = muted_for_secs.unwrap_or(DEFAULT_MUTE_SECS);
            anyhow::ensure!(
                (60..=MAX_MUTE_SECS).contains(&seconds),
                "fleet_alert_mute_duration_invalid"
            );
        }
        ACTION_ACKNOWLEDGE | ACTION_ESCALATE | ACTION_CLEAR => {
            anyhow::ensure!(
                muted_for_secs.is_none(),
                "fleet_alert_mute_duration_unexpected"
            );
        }
        _ => anyhow::bail!("fleet_alert_state_action_invalid"),
    }
    Ok(())
}

fn validate_alert_id(alert_id: &str) -> Result<()> {
    let alert_id = alert_id.trim();
    anyhow::ensure!(
        !alert_id.is_empty() && alert_id.len() <= MAX_ALERT_ID_BYTES,
        "fleet alert id must be between 1 and 192 bytes"
    );
    anyhow::ensure!(
        alert_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.')
        }),
        "fleet alert id contains unsupported characters"
    );
    Ok(())
}

fn validate_alert_reason(reason: Option<&str>) -> Result<()> {
    if let Some(reason) = reason {
        anyhow::ensure!(
            reason.len() <= MAX_ALERT_REASON_BYTES,
            "fleet alert state reason is too long"
        );
    }
    Ok(())
}

fn normalize_optional_state(state: Option<&str>) -> Result<Option<String>> {
    state
        .map(str::trim)
        .filter(|state| !state.is_empty())
        .map(|state| {
            normalize_state(state)
                .map(ToOwned::to_owned)
                .context("invalid fleet alert state")
        })
        .transpose()
}

fn normalize_state(state: &str) -> Result<&'static str> {
    match state.trim() {
        ALERT_STATE_OPEN => Ok(ALERT_STATE_OPEN),
        ALERT_STATE_ACKNOWLEDGED => Ok(ALERT_STATE_ACKNOWLEDGED),
        ALERT_STATE_MUTED => Ok(ALERT_STATE_MUTED),
        ALERT_STATE_ESCALATED => Ok(ALERT_STATE_ESCALATED),
        _ => anyhow::bail!("fleet alert state must be open, acknowledged, muted, or escalated"),
    }
}

fn sort_alert_states(states: &mut [FleetAlertStateView]) {
    states.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.alert_id.cmp(&right.alert_id))
    });
}

fn alert_state_from_row(row: sqlx::postgres::PgRow) -> Result<FleetAlertStateView> {
    Ok(FleetAlertStateView {
        alert_id: row.try_get("alert_id")?,
        state: row.try_get("state")?,
        muted_until_unix: row.try_get("muted_until_unix")?,
        escalation_level: row.try_get("escalation_level")?,
        revision: row.try_get("revision")?,
        reason: row.try_get("reason")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn alert_state_audit(
    state: &FleetAlertStateView,
    operator: &AuthContext,
    created_at: String,
    batch_id: Uuid,
    batch_size: usize,
    batch_action: &str,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "fleet.alert_state_updated".to_string(),
        target: format!("fleet_alert:{}", state.alert_id),
        command_hash: None,
        metadata: alert_state_metadata(state, operator, batch_id, batch_size, batch_action),
        created_at,
    }
}

fn alert_state_metadata(
    state: &FleetAlertStateView,
    operator: &AuthContext,
    batch_id: Uuid,
    batch_size: usize,
    batch_action: &str,
) -> serde_json::Value {
    json!({
        "alert_id": state.alert_id,
        "state": state.state,
        "muted_until_unix": state.muted_until_unix,
        "escalation_level": state.escalation_level,
        "revision": state.revision,
        "reason": state.reason,
        "batch_id": batch_id,
        "batch_size": batch_size,
        "batch_action": batch_action,
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": operator.operator.username,
        "operator_role": operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "alert-state-controller",
    })
}
