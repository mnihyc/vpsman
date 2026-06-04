use anyhow::{Context, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{AuditLogView, AuthContext},
    model_alert_states::{FleetAlertStateView, UpdateFleetAlertStateRequest},
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

    pub(crate) async fn update_fleet_alert_state(
        &self,
        request: &UpdateFleetAlertStateRequest,
        operator: &AuthContext,
    ) -> Result<FleetAlertStateView> {
        let next = alert_state_from_request(request, operator)?;
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut states = memory.fleet_alert_states.write().await;
                let state = if let Some(stored) = states
                    .iter_mut()
                    .find(|stored| stored.alert_id == next.alert_id)
                {
                    stored.state = next.state.clone();
                    stored.muted_until_unix = next.muted_until_unix;
                    stored.escalation_level = next.escalation_level;
                    stored.reason = next.reason.clone();
                    stored.actor_id = next.actor_id;
                    stored.updated_at = now.clone();
                    stored.clone()
                } else {
                    states.push(next.clone());
                    next
                };
                drop(states);
                memory
                    .audits
                    .write()
                    .await
                    .push(alert_state_audit(&state, operator, now));
                Ok(state)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO fleet_alert_states (
                        alert_id,
                        state,
                        muted_until_unix,
                        escalation_level,
                        reason,
                        actor_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    ON CONFLICT (alert_id) DO UPDATE SET
                        state = EXCLUDED.state,
                        muted_until_unix = EXCLUDED.muted_until_unix,
                        escalation_level = EXCLUDED.escalation_level,
                        reason = EXCLUDED.reason,
                        actor_id = EXCLUDED.actor_id,
                        updated_at = now()
                    RETURNING
                        alert_id,
                        state,
                        muted_until_unix,
                        escalation_level,
                        reason,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(&next.alert_id)
                .bind(&next.state)
                .bind(next.muted_until_unix)
                .bind(next.escalation_level)
                .bind(&next.reason)
                .bind(operator.operator.id)
                .fetch_one(&mut *tx)
                .await?;
                let state = alert_state_from_row(row)?;
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
                .bind("fleet.alert_state_updated")
                .bind(format!("fleet_alert:{}", state.alert_id))
                .bind(alert_state_metadata(&state, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(state)
            }
        }
    }
}

fn alert_state_from_request(
    request: &UpdateFleetAlertStateRequest,
    operator: &AuthContext,
) -> Result<FleetAlertStateView> {
    anyhow::ensure!(request.confirmed, "fleet_alert_state_confirmation_required");
    validate_alert_id(&request.alert_id)?;
    validate_alert_reason(request.reason.as_deref())?;
    let alert_id = request.alert_id.trim().to_string();
    let action = request.action.trim();
    let now = unix_now() as i64;
    let current_escalation = 0;
    let (state, muted_until_unix, escalation_level) = match action {
        ACTION_ACKNOWLEDGE => (
            ALERT_STATE_ACKNOWLEDGED.to_string(),
            None,
            current_escalation,
        ),
        ACTION_MUTE => {
            let seconds = request.muted_for_secs.unwrap_or(DEFAULT_MUTE_SECS);
            anyhow::ensure!(
                (60..=MAX_MUTE_SECS).contains(&seconds),
                "fleet_alert_mute_duration_invalid"
            );
            (
                ALERT_STATE_MUTED.to_string(),
                Some(now.saturating_add(seconds)),
                current_escalation,
            )
        }
        ACTION_ESCALATE => (
            ALERT_STATE_ESCALATED.to_string(),
            None,
            current_escalation + 1,
        ),
        ACTION_CLEAR => (ALERT_STATE_OPEN.to_string(), None, 0),
        _ => anyhow::bail!("fleet_alert_state_action_invalid"),
    };
    Ok(FleetAlertStateView {
        alert_id,
        state,
        muted_until_unix,
        escalation_level,
        reason: request
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(ToOwned::to_owned),
        actor_id: Some(operator.operator.id),
        created_at: now.to_string(),
        updated_at: now.to_string(),
    })
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
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "fleet.alert_state_updated".to_string(),
        target: format!("fleet_alert:{}", state.alert_id),
        command_hash: None,
        metadata: alert_state_metadata(state, operator),
        created_at,
    }
}

fn alert_state_metadata(state: &FleetAlertStateView, operator: &AuthContext) -> serde_json::Value {
    json!({
        "alert_id": state.alert_id,
        "state": state.state,
        "muted_until_unix": state.muted_until_unix,
        "escalation_level": state.escalation_level,
        "reason": state.reason,
        "operator_id": operator.operator.id,
        "operator_username": operator.operator.username,
        "session_id": operator.session_id,
    })
}
