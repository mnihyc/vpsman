use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;

use crate::{
    model::{AgentView, FleetAlertQuery, FleetAlertView},
    model_alert_notifications::FleetAlertNotificationMatchRule,
    model_alert_policies::PolicyAlertQuery,
    model_alert_states::FleetAlertStateView,
    repository_alert_policies::policy_alert_to_fleet_alert,
    repository_operational_alerts::{
        operational_episode_to_fleet_alert, OPERATIONAL_ALERT_SOURCE_LIMIT,
    },
    state::AppState,
    unix_now,
    util::{compare_timestamps_desc, timestamp_in_optional_bounds},
};

const FLEET_ALERT_RESULT_LIMIT_MAX: i64 = 200;
// Historical/event sources are deliberately bounded independently before they
// are merged with current agent snapshots. Repository
// selectors must apply native client, category, severity, and dashboard-window
// filters before this horizon so a narrow query is not crowded out by unrelated
// fleet history. Saturation is surfaced to dashboard/UI consumers as a lower
// bound; older event history remains available from its owning workflow.
const FLEET_EVENT_SOURCE_HORIZON_MAX: i64 = 200;

pub(crate) fn fleet_alert_is_confirmed_active(alert: &FleetAlertView) -> bool {
    matches!(alert.lifecycle.state.as_str(), "triggered" | "persisting")
}

#[derive(Clone, Debug)]
pub(crate) struct AgentAlertScope {
    pub(crate) provider: Option<String>,
    pub(crate) tags: Vec<String>,
}

pub(crate) struct FleetAlertSelector<'a> {
    pub(crate) allowed_client_ids: &'a HashSet<String>,
    pub(crate) end_unix: u64,
    pub(crate) include_global: bool,
}

pub(crate) struct FleetAlertSelection {
    pub(crate) alerts: Vec<FleetAlertView>,
    pub(crate) truncated: bool,
}

#[derive(Clone, Copy)]
enum PolicyAlertSource {
    CurrentFleet,
    ConfirmedActive,
}

pub(crate) fn build_agent_alert_scopes(agents: &[AgentView]) -> HashMap<String, AgentAlertScope> {
    agents
        .iter()
        .map(|agent| {
            (
                agent.id.clone(),
                AgentAlertScope {
                    provider: provider_from_agent(agent),
                    tags: agent.tags.clone(),
                },
            )
        })
        .collect()
}

fn provider_from_agent(agent: &AgentView) -> Option<String> {
    agent.tags.iter().find_map(|tag| {
        tag.strip_prefix("provider:")
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    })
}

impl AppState {
    pub(crate) async fn list_fleet_alerts(
        &self,
        query: FleetAlertQuery,
    ) -> Result<Vec<FleetAlertView>> {
        Ok(self
            .list_fleet_alerts_selected_with_policy_source(
                query,
                None,
                PolicyAlertSource::CurrentFleet,
                None,
            )
            .await?
            .alerts)
    }

    pub(crate) async fn list_fleet_alerts_for_notification_dispatch(
        &self,
        query: FleetAlertQuery,
        notification_rules: &[FleetAlertNotificationMatchRule],
    ) -> Result<Vec<FleetAlertView>> {
        Ok(self
            .list_fleet_alerts_selected_with_policy_source(
                query,
                None,
                PolicyAlertSource::ConfirmedActive,
                Some(notification_rules),
            )
            .await?
            .alerts)
    }

    pub(crate) async fn list_fleet_alert_history(
        &self,
        query: FleetAlertQuery,
    ) -> Result<FleetAlertSelection> {
        self.list_fleet_alert_history_bounded(query, None, None)
            .await
    }

    pub(crate) async fn list_fleet_alert_history_bounded(
        &self,
        query: FleetAlertQuery,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
    ) -> Result<FleetAlertSelection> {
        let operational = self
            .repo
            .list_operational_alert_episodes(
                &query,
                true,
                false,
                None,
                None,
                true,
                start_unix,
                end_unix,
                OPERATIONAL_ALERT_SOURCE_LIMIT,
                None,
            )
            .await?;
        let policy_query = PolicyAlertQuery {
            limit: Some(OPERATIONAL_ALERT_SOURCE_LIMIT as i64),
            client_id: query.client_id.clone(),
            severity: query.severity.clone(),
            category: query.category.clone(),
            policy_group_id: None,
        };
        let policy = self
            .repo
            .list_policy_alert_fleet_history(
                &policy_query,
                OPERATIONAL_ALERT_SOURCE_LIMIT,
                start_unix,
                end_unix,
                query.operator_state.as_deref(),
                query.include_muted.unwrap_or(false),
            )
            .await?;
        let mut alerts = operational
            .iter()
            .take(FLEET_EVENT_SOURCE_HORIZON_MAX as usize)
            .map(operational_episode_to_fleet_alert)
            .chain(
                policy
                    .iter()
                    .take(FLEET_EVENT_SOURCE_HORIZON_MAX as usize)
                    .map(policy_alert_to_fleet_alert),
            )
            .collect::<Vec<_>>();
        let alert_ids = alerts
            .iter()
            .map(|alert| alert.id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let states = self
            .repo
            .list_fleet_alert_states_for_alert_ids(&alert_ids)
            .await?;
        apply_alert_states(&mut alerts, &states);
        let result_truncated = apply_alert_history_filters(&mut alerts, &query);
        Ok(FleetAlertSelection {
            alerts,
            truncated: operational.len() >= OPERATIONAL_ALERT_SOURCE_LIMIT
                || policy.len() >= OPERATIONAL_ALERT_SOURCE_LIMIT
                || result_truncated,
        })
    }

    pub(crate) async fn list_fleet_alerts_selected(
        &self,
        query: FleetAlertQuery,
        selector: Option<FleetAlertSelector<'_>>,
    ) -> Result<FleetAlertSelection> {
        self.list_fleet_alerts_selected_with_policy_source(
            query,
            selector,
            PolicyAlertSource::CurrentFleet,
            None,
        )
        .await
    }

    async fn list_fleet_alerts_selected_with_policy_source(
        &self,
        query: FleetAlertQuery,
        selector: Option<FleetAlertSelector<'_>>,
        policy_alert_source: PolicyAlertSource,
        notification_rules: Option<&[FleetAlertNotificationMatchRule]>,
    ) -> Result<FleetAlertSelection> {
        let selector = selector.as_ref();
        let mut alerts = Vec::new();
        let mut source_saturated = false;
        let visible_agents = self.repo.list_agents().await?;
        let visible_client_ids = visible_agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let operational_client_ids = selector
            .map(|selector| {
                selector
                    .allowed_client_ids
                    .intersection(&visible_client_ids)
                    .cloned()
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_else(|| visible_client_ids.clone());
        let confirmed_active = matches!(policy_alert_source, PolicyAlertSource::ConfirmedActive);
        let record_kind_cohorts: &[Option<&str>] = if notification_rules.is_some() {
            &[Some("condition"), Some("event")]
        } else {
            &[None]
        };
        for record_kind in record_kind_cohorts {
            let operational = self
                .repo
                .list_operational_alert_episodes(
                    &query,
                    false,
                    confirmed_active,
                    *record_kind,
                    selector.map(|selector| selector.allowed_client_ids),
                    selector.is_none_or(|selector| selector.include_global),
                    None,
                    selector.map(|selector| selector.end_unix),
                    OPERATIONAL_ALERT_SOURCE_LIMIT,
                    notification_rules,
                )
                .await?;
            source_saturated |= operational.len() >= OPERATIONAL_ALERT_SOURCE_LIMIT;
            alerts.extend(
                operational
                    .iter()
                    .take(FLEET_EVENT_SOURCE_HORIZON_MAX as usize)
                    .map(operational_episode_to_fleet_alert),
            );
        }

        let policy_query = PolicyAlertQuery {
            limit: None,
            client_id: query.client_id.clone(),
            severity: query.severity.clone(),
            category: query.category.clone(),
            policy_group_id: None,
        };
        let policy_alerts = self
            .repo
            .list_policy_alert_fleet_candidates(
                &policy_query,
                FLEET_EVENT_SOURCE_HORIZON_MAX as usize,
                matches!(policy_alert_source, PolicyAlertSource::ConfirmedActive),
                Some(&operational_client_ids),
                None,
                selector.map(|selector| selector.end_unix),
                query.operator_state.as_deref(),
                query.include_muted.unwrap_or(false),
                notification_rules,
            )
            .await?;
        source_saturated |= policy_alerts.len() >= FLEET_EVENT_SOURCE_HORIZON_MAX as usize;
        alerts.extend(policy_alerts.iter().map(policy_alert_to_fleet_alert));

        let alert_ids = alerts
            .iter()
            .map(|alert| alert.id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let alert_states = self
            .repo
            .list_fleet_alert_states_for_alert_ids(&alert_ids)
            .await?;
        apply_alert_states(&mut alerts, &alert_states);
        if let Some(selector) = selector {
            apply_alert_selector(&mut alerts, selector);
        }
        let result_truncated =
            apply_alert_filters(&mut alerts, &query, notification_rules.is_none());
        Ok(FleetAlertSelection {
            alerts,
            truncated: source_saturated || result_truncated,
        })
    }
}

pub(crate) fn apply_alert_states(alerts: &mut [FleetAlertView], states: &[FleetAlertStateView]) {
    let now = unix_now() as i64;
    let state_by_id = states
        .iter()
        .map(|state| (state.alert_id.as_str(), state))
        .collect::<HashMap<_, _>>();
    for alert in alerts {
        let Some(state) = state_by_id.get(alert.id.as_str()) else {
            continue;
        };
        let effective_state = if state.state == "muted" {
            match state.muted_until_unix {
                Some(until) if until > now => "muted",
                _ => "open",
            }
        } else {
            state.state.as_str()
        };
        alert.operator_state = effective_state.to_string();
        alert.muted_until_unix = state.muted_until_unix;
        alert.escalation_level = state.escalation_level;
        alert.state_reason = state.reason.clone();
        alert.state_actor_id = state.actor_id;
        alert.state_updated_at = Some(state.updated_at.clone());
    }
}

fn apply_alert_selector(alerts: &mut Vec<FleetAlertView>, selector: &FleetAlertSelector<'_>) {
    alerts.retain(|alert| {
        let client_matches = alert
            .client_id
            .as_ref()
            .map(|client_id| selector.allowed_client_ids.contains(client_id))
            .unwrap_or(selector.include_global);
        client_matches
            && timestamp_in_optional_bounds(&alert.observed_at, None, Some(selector.end_unix))
    });
}

fn apply_alert_filters(
    alerts: &mut Vec<FleetAlertView>,
    query: &FleetAlertQuery,
    conditions_first: bool,
) -> bool {
    if let Some(client_id) = query.client_id.as_deref() {
        alerts.retain(|alert| alert.client_id.as_deref() == Some(client_id));
    }
    if let Some(severity) = query.severity.as_deref() {
        alerts.retain(|alert| alert.severity == severity);
    }
    if let Some(category) = query.category.as_deref() {
        alerts.retain(|alert| alert.category == category);
    }
    if !query.include_muted.unwrap_or(false) {
        alerts.retain(|alert| alert.operator_state != "muted");
    }
    if let Some(operator_state) = query.operator_state.as_deref() {
        alerts.retain(|alert| alert.operator_state == operator_state);
    }
    alerts.sort_by(|left, right| {
        (if conditions_first {
            record_kind_rank(&left.record_kind).cmp(&record_kind_rank(&right.record_kind))
        } else {
            std::cmp::Ordering::Equal
        })
        .then_with(|| {
            if conditions_first {
                lifecycle_rank(&left.lifecycle.state).cmp(&lifecycle_rank(&right.lifecycle.state))
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .then_with(|| {
            operator_state_rank(&left.operator_state)
                .cmp(&operator_state_rank(&right.operator_state))
        })
        .then_with(|| severity_rank(&left.severity).cmp(&severity_rank(&right.severity)))
        .then_with(|| right.escalation_level.cmp(&left.escalation_level))
        .then_with(|| compare_timestamps_desc(&left.observed_at, &right.observed_at))
        .then_with(|| left.category.cmp(&right.category))
        .then_with(|| left.target_id.cmp(&right.target_id))
    });
    let limit = query
        .limit
        .unwrap_or(50)
        .clamp(1, FLEET_ALERT_RESULT_LIMIT_MAX) as usize;
    let truncated = alerts.len() > limit;
    alerts.truncate(limit);
    truncated
}

fn apply_alert_history_filters(alerts: &mut Vec<FleetAlertView>, query: &FleetAlertQuery) -> bool {
    if let Some(client_id) = query.client_id.as_deref() {
        alerts.retain(|alert| alert.client_id.as_deref() == Some(client_id));
    }
    if let Some(severity) = query.severity.as_deref() {
        alerts.retain(|alert| alert.severity == severity);
    }
    if let Some(category) = query.category.as_deref() {
        alerts.retain(|alert| alert.category == category);
    }
    if !query.include_muted.unwrap_or(false) {
        alerts.retain(|alert| alert.operator_state != "muted");
    }
    if let Some(operator_state) = query.operator_state.as_deref() {
        alerts.retain(|alert| alert.operator_state == operator_state);
    }
    alerts.sort_by(|left, right| {
        compare_timestamps_desc(&left.lifecycle.triggered_at, &right.lifecycle.triggered_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    let limit = query
        .limit
        .unwrap_or(50)
        .clamp(1, FLEET_ALERT_RESULT_LIMIT_MAX) as usize;
    let truncated = alerts.len() > limit;
    alerts.truncate(limit);
    truncated
}

fn record_kind_rank(kind: &str) -> usize {
    usize::from(kind != "condition")
}

fn lifecycle_rank(state: &str) -> usize {
    match state {
        "triggered" | "persisting" => 0,
        "unknown" => 1,
        _ => 2,
    }
}

fn operator_state_rank(state: &str) -> usize {
    match state {
        "escalated" => 0,
        "open" => 1,
        "acknowledged" => 2,
        "muted" => 3,
        _ => 4,
    }
}

fn severity_rank(severity: &str) -> usize {
    match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}
