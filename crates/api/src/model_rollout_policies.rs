use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AgentUpdateRolloutPolicyView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    pub(crate) scope_value: Option<String>,
    pub(crate) channel: Option<String>,
    pub(crate) canary_count: Option<i32>,
    pub(crate) automation_health_gate: Option<String>,
    pub(crate) priority: i32,
    pub(crate) enabled: bool,
    pub(crate) notes: Option<String>,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AgentUpdateRolloutPolicyQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) enabled: Option<bool>,
    pub(crate) channel: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CreateAgentUpdateRolloutPolicyRequest {
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    #[serde(default)]
    pub(crate) scope_value: Option<String>,
    #[serde(default)]
    pub(crate) channel: Option<String>,
    #[serde(default)]
    pub(crate) canary_count: Option<i32>,
    #[serde(default)]
    pub(crate) automation_health_gate: Option<String>,
    #[serde(default)]
    pub(crate) priority: i32,
    #[serde(default = "default_rollout_policy_enabled")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) notes: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ResolvedAgentUpdateRolloutPolicy {
    pub(crate) policy_id: Option<Uuid>,
    pub(crate) policy_name: Option<String>,
    pub(crate) canary_count: Option<i32>,
    pub(crate) automation_health_gate: Option<String>,
}

fn default_rollout_policy_enabled() -> bool {
    true
}
