use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::AgentView;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebhookRuleView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) expression: String,
    pub(crate) target: String,
    pub(crate) body_template: String,
    pub(crate) cooldown_secs: i64,
    pub(crate) notes: Option<String>,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WebhookRuleQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateWebhookRuleRequest {
    pub(crate) id: Option<Uuid>,
    pub(crate) name: String,
    #[serde(default = "default_webhook_rule_enabled")]
    pub(crate) enabled: bool,
    pub(crate) expression: String,
    pub(crate) target: String,
    #[serde(default)]
    pub(crate) body_template: String,
    pub(crate) cooldown_secs: Option<i64>,
    pub(crate) notes: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebhookRuleDeliveryView {
    pub(crate) id: Uuid,
    pub(crate) rule_id: Uuid,
    pub(crate) rule_name: String,
    pub(crate) event_kind: String,
    pub(crate) event_id: String,
    pub(crate) status: String,
    pub(crate) target: String,
    pub(crate) dedupe_key: String,
    pub(crate) payload: serde_json::Value,
    pub(crate) matched_vps: Vec<AgentView>,
    pub(crate) message: String,
    pub(crate) error: Option<String>,
    pub(crate) cooldown_until_unix: i64,
    pub(crate) attempt_count: i32,
    pub(crate) next_attempt_at: Option<String>,
    pub(crate) last_attempt_at: Option<String>,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) delivered_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebhookRuleDryRunView {
    pub(crate) rendered_message: String,
    pub(crate) matched_vps: Vec<AgentView>,
    pub(crate) payload_context: serde_json::Value,
    pub(crate) validation_errors: Vec<String>,
    pub(crate) delivery: Option<WebhookRuleDeliveryView>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WebhookRuleDeliveryQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) rule_id: Option<Uuid>,
    pub(crate) event_kind: Option<String>,
    pub(crate) status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebhookRuleDryRunRequest {
    pub(crate) name: Option<String>,
    pub(crate) enabled: Option<bool>,
    pub(crate) expression: String,
    pub(crate) target: Option<String>,
    #[serde(default = "default_webhook_dry_run_event_kind")]
    pub(crate) event_kind: String,
    pub(crate) event_id: Option<String>,
    #[serde(default)]
    pub(crate) body_template: String,
    pub(crate) cooldown_secs: Option<i64>,
    pub(crate) notes: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebhookRuleDispatchRequest {
    #[serde(default = "default_webhook_dry_run_event_kind")]
    pub(crate) event_kind: String,
    pub(crate) event_id: Option<String>,
    pub(crate) limit: Option<i64>,
    pub(crate) dry_run: Option<bool>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebhookRuleProcessRequest {
    pub(crate) limit: Option<i64>,
    pub(crate) status: Option<String>,
    pub(crate) dry_run: Option<bool>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebhookDeliveryRotationRequest {
    pub(crate) older_than: Option<String>,
    pub(crate) older_than_days: Option<i64>,
    pub(crate) status: Option<String>,
    pub(crate) rule_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebhookDeliveryRotationResponse {
    pub(crate) matched_count: usize,
    pub(crate) deleted_count: usize,
    pub(crate) confirmation_required: bool,
    pub(crate) older_than: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) rule_id: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub(crate) struct WebhookEventCandidate {
    pub(crate) kind: String,
    pub(crate) event_id: String,
    pub(crate) event_predicates: Vec<String>,
    pub(crate) subject_client_ids: Vec<String>,
    pub(crate) payload: serde_json::Value,
    pub(crate) actor_id: Option<Uuid>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct WebhookEventRow {
    pub(crate) id: Uuid,
    pub(crate) kind: String,
    pub(crate) event_id: String,
    pub(crate) event_predicates: Vec<String>,
    pub(crate) subject_client_ids: Vec<String>,
    pub(crate) payload: serde_json::Value,
    pub(crate) occurred_at: String,
    pub(crate) actor_id: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub(crate) struct WebhookRuleDeliveryCandidate {
    pub(crate) rule_id: Uuid,
    pub(crate) rule_name: String,
    pub(crate) event_kind: String,
    pub(crate) event_id: String,
    pub(crate) target: String,
    pub(crate) dedupe_key: String,
    pub(crate) payload: serde_json::Value,
    pub(crate) matched_vps: Vec<AgentView>,
    pub(crate) message: String,
    pub(crate) cooldown_until_unix: i64,
    pub(crate) actor_id: Option<Uuid>,
}

fn default_webhook_rule_enabled() -> bool {
    true
}

fn default_webhook_dry_run_event_kind() -> String {
    "manual.dry_run".to_string()
}
