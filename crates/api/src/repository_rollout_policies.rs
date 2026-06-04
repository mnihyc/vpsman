use std::collections::HashSet;

use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::{
    model::{AgentView, AuditLogView, AuthContext, CreateJobRequest, ResourcePoolView},
    model_rollout_policies::{
        AgentUpdateRolloutPolicyView, CreateAgentUpdateRolloutPolicyRequest,
        ResolvedAgentUpdateRolloutPolicy,
    },
    repository::Repository,
    unix_now,
};

const POLICY_SCOPE_GLOBAL: &str = "global";
const POLICY_SCOPE_TAG: &str = "tag";
const POLICY_SCOPE_POOL: &str = "pool";
const POLICY_SCOPE_PROVIDER: &str = "provider";

impl Repository {
    pub(crate) async fn list_agent_update_rollout_policies(
        &self,
        limit: i64,
        enabled: Option<bool>,
        channel: Option<&str>,
    ) -> Result<Vec<AgentUpdateRolloutPolicyView>> {
        let channel = channel
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        match self {
            Self::Memory(memory) => {
                let mut policies = memory
                    .agent_update_rollout_policies
                    .read()
                    .await
                    .iter()
                    .filter(|policy| enabled.is_none_or(|value| policy.enabled == value))
                    .filter(|policy| {
                        channel
                            .as_deref()
                            .is_none_or(|value| policy.channel.as_deref() == Some(value))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                policies.sort_by(|left, right| {
                    right
                        .enabled
                        .cmp(&left.enabled)
                        .then_with(|| right.priority.cmp(&left.priority))
                        .then_with(|| left.scope_kind.cmp(&right.scope_kind))
                        .then_with(|| left.name.cmp(&right.name))
                });
                policies.truncate(limit as usize);
                Ok(policies)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        channel,
                        canary_count,
                        automation_health_gate,
                        priority,
                        enabled,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM agent_update_rollout_policies
                    WHERE ($2::boolean IS NULL OR enabled = $2)
                      AND ($3::text IS NULL OR channel = $3)
                    ORDER BY enabled DESC, priority DESC, scope_kind, name
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .bind(enabled)
                .bind(channel.as_deref())
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(policy_from_row).collect()
            }
        }
    }

    pub(crate) async fn upsert_agent_update_rollout_policy(
        &self,
        request: &CreateAgentUpdateRolloutPolicyRequest,
        operator: &AuthContext,
    ) -> Result<AgentUpdateRolloutPolicyView> {
        let candidate = rollout_policy_from_request(request, operator);
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut policies = memory.agent_update_rollout_policies.write().await;
                let policy = if let Some(stored) = policies
                    .iter_mut()
                    .find(|stored| stored.name == candidate.name)
                {
                    stored.scope_kind = candidate.scope_kind.clone();
                    stored.scope_value = candidate.scope_value.clone();
                    stored.channel = candidate.channel.clone();
                    stored.canary_count = candidate.canary_count;
                    stored.automation_health_gate = candidate.automation_health_gate.clone();
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
                memory.audits.write().await.push(policy_audit(
                    &policy,
                    operator,
                    "agent_update.rollout_policy_upserted",
                    now,
                ));
                Ok(policy)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO agent_update_rollout_policies (
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        channel,
                        canary_count,
                        automation_health_gate,
                        priority,
                        enabled,
                        notes,
                        actor_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                    ON CONFLICT (name) DO UPDATE SET
                        scope_kind = EXCLUDED.scope_kind,
                        scope_value = EXCLUDED.scope_value,
                        channel = EXCLUDED.channel,
                        canary_count = EXCLUDED.canary_count,
                        automation_health_gate = EXCLUDED.automation_health_gate,
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
                        channel,
                        canary_count,
                        automation_health_gate,
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
                .bind(&candidate.channel)
                .bind(candidate.canary_count)
                .bind(&candidate.automation_health_gate)
                .bind(candidate.priority)
                .bind(candidate.enabled)
                .bind(&candidate.notes)
                .bind(operator.operator.id)
                .fetch_one(&mut *tx)
                .await?;
                let policy = policy_from_row(row)?;
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
                .bind("agent_update.rollout_policy_upserted")
                .bind(format!("agent_update_rollout_policy:{}", policy.id))
                .bind(policy_metadata(&policy, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(policy)
            }
        }
    }

    pub(crate) async fn resolve_agent_update_rollout_policy(
        &self,
        request: &CreateJobRequest,
        job_command: &JobCommand,
        resolved_agents: &[AgentView],
    ) -> Result<ResolvedAgentUpdateRolloutPolicy> {
        let JobCommand::UpdateAgent {
            sha256_hex,
            artifact_signing_key_hex,
            ..
        } = job_command
        else {
            return Ok(ResolvedAgentUpdateRolloutPolicy::default());
        };
        let release_channel = self
            .find_agent_update_release_for_artifact(sha256_hex, artifact_signing_key_hex.as_deref())
            .await?
            .map(|release| release.channel);
        let policies = self
            .list_agent_update_rollout_policies(1000, Some(true), None)
            .await?;
        let pools = self.list_pools().await?;
        let mut matches = policies
            .into_iter()
            .filter_map(|policy| {
                policy_match_score(
                    &policy,
                    request,
                    resolved_agents,
                    &pools,
                    release_channel.as_deref(),
                )
                .map(|score| (policy, score))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left, left_score), (right, right_score)| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| right_score.cmp(left_score))
                .then_with(|| right.updated_at.cmp(&left.updated_at))
                .then_with(|| right.id.cmp(&left.id))
        });
        let Some((policy, _score)) = matches.into_iter().next() else {
            return Ok(ResolvedAgentUpdateRolloutPolicy::default());
        };
        Ok(ResolvedAgentUpdateRolloutPolicy {
            policy_id: Some(policy.id),
            policy_name: Some(policy.name),
            canary_count: policy.canary_count,
            automation_health_gate: policy.automation_health_gate,
        })
    }
}

fn rollout_policy_from_request(
    request: &CreateAgentUpdateRolloutPolicyRequest,
    operator: &AuthContext,
) -> AgentUpdateRolloutPolicyView {
    let now = unix_now().to_string();
    AgentUpdateRolloutPolicyView {
        id: Uuid::new_v4(),
        name: request.name.trim().to_string(),
        scope_kind: request.scope_kind.trim().to_ascii_lowercase(),
        scope_value: trimmed_optional(request.scope_value.as_deref()),
        channel: trimmed_optional(request.channel.as_deref())
            .map(|value| value.to_ascii_lowercase()),
        canary_count: request.canary_count,
        automation_health_gate: trimmed_optional(request.automation_health_gate.as_deref()),
        priority: request.priority,
        enabled: request.enabled,
        notes: trimmed_optional(request.notes.as_deref()),
        actor_id: Some(operator.operator.id),
        created_at: now.clone(),
        updated_at: now,
    }
}

fn policy_match_score(
    policy: &AgentUpdateRolloutPolicyView,
    request: &CreateJobRequest,
    resolved_agents: &[AgentView],
    pools: &[ResourcePoolView],
    release_channel: Option<&str>,
) -> Option<i32> {
    if !policy.enabled || !policy_channel_matches(policy, release_channel) {
        return None;
    }
    match policy.scope_kind.as_str() {
        POLICY_SCOPE_GLOBAL => Some(0),
        POLICY_SCOPE_TAG => {
            let value = policy.scope_value.as_deref()?;
            tag_scope_matches(value, request, resolved_agents).then_some(30)
        }
        POLICY_SCOPE_POOL => {
            let value = policy.scope_value.as_deref()?;
            pool_scope_matches(value, request, resolved_agents, pools).then_some(40)
        }
        POLICY_SCOPE_PROVIDER => {
            let value = policy.scope_value.as_deref()?;
            provider_scope_matches(value, request, resolved_agents, pools).then_some(20)
        }
        _ => None,
    }
}

fn policy_channel_matches(
    policy: &AgentUpdateRolloutPolicyView,
    release_channel: Option<&str>,
) -> bool {
    policy.channel.as_deref().is_none_or(|channel| {
        release_channel.is_some_and(|release_channel| channel == release_channel)
    })
}

fn tag_scope_matches(
    scope_value: &str,
    request: &CreateJobRequest,
    resolved_agents: &[AgentView],
) -> bool {
    request
        .tags
        .iter()
        .any(|tag| tag.eq_ignore_ascii_case(scope_value))
        || resolved_agents.iter().any(|agent| {
            agent
                .tags
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case(scope_value))
        })
}

fn pool_scope_matches(
    scope_value: &str,
    request: &CreateJobRequest,
    resolved_agents: &[AgentView],
    pools: &[ResourcePoolView],
) -> bool {
    let requested_pool_ids = request
        .pools
        .iter()
        .map(Uuid::to_string)
        .collect::<HashSet<_>>();
    let target_ids = resolved_agents
        .iter()
        .map(|agent| agent.id.as_str())
        .collect::<HashSet<_>>();
    pools.iter().any(|pool| {
        (pool.id.to_string() == scope_value || pool.name.eq_ignore_ascii_case(scope_value))
            && (requested_pool_ids.contains(&pool.id.to_string())
                || pool
                    .clients
                    .iter()
                    .any(|agent| target_ids.contains(agent.id.as_str())))
    })
}

fn provider_scope_matches(
    scope_value: &str,
    request: &CreateJobRequest,
    resolved_agents: &[AgentView],
    pools: &[ResourcePoolView],
) -> bool {
    let requested_pool_ids = request
        .pools
        .iter()
        .map(Uuid::to_string)
        .collect::<HashSet<_>>();
    let target_ids = resolved_agents
        .iter()
        .map(|agent| agent.id.as_str())
        .collect::<HashSet<_>>();
    pools.iter().any(|pool| {
        pool.provider
            .as_deref()
            .is_some_and(|provider| provider.eq_ignore_ascii_case(scope_value))
            && (requested_pool_ids.contains(&pool.id.to_string())
                || pool
                    .clients
                    .iter()
                    .any(|agent| target_ids.contains(agent.id.as_str())))
    })
}

fn trimmed_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn policy_from_row(row: sqlx::postgres::PgRow) -> Result<AgentUpdateRolloutPolicyView> {
    Ok(AgentUpdateRolloutPolicyView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        scope_kind: row.try_get("scope_kind")?,
        scope_value: row.try_get("scope_value")?,
        channel: row.try_get("channel")?,
        canary_count: row.try_get("canary_count")?,
        automation_health_gate: row.try_get("automation_health_gate")?,
        priority: row.try_get("priority")?,
        enabled: row.try_get("enabled")?,
        notes: row.try_get("notes")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn policy_audit(
    policy: &AgentUpdateRolloutPolicyView,
    operator: &AuthContext,
    action: &str,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: format!("agent_update_rollout_policy:{}", policy.id),
        command_hash: None,
        metadata: policy_metadata(policy, operator),
        created_at,
    }
}

fn policy_metadata(
    policy: &AgentUpdateRolloutPolicyView,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "policy_id": policy.id,
        "name": &policy.name,
        "scope_kind": &policy.scope_kind,
        "scope_value": &policy.scope_value,
        "channel": &policy.channel,
        "canary_count": policy.canary_count,
        "automation_health_gate": &policy.automation_health_gate,
        "priority": policy.priority,
        "enabled": policy.enabled,
        "operator_id": operator.operator.id,
        "operator_username": operator.operator.username,
        "operator_role": operator.operator.role,
        "session_id": operator.session_id,
    })
}
