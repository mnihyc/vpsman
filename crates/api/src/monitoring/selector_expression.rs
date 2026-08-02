use crate::model::AgentView;
use vpsman_common::{Expression, ExpressionContext, VpsMetadata};

pub(crate) fn parse_selector_expression(input: &str) -> Result<Option<Expression>, String> {
    vpsman_common::parse_expression(input)
}

pub(crate) fn agent_matches_selector_expression(
    agent: &AgentView,
    expression: &Expression,
) -> bool {
    vpsman_common::expression_matches(&agent_expression_context(agent), expression)
}

pub(crate) fn agent_expression_context(agent: &AgentView) -> ExpressionContext {
    ExpressionContext::for_vps(VpsMetadata {
        id: agent.id.clone(),
        display_name: agent.display_name.clone(),
        status: agent.status.clone(),
        tags: agent.tags.clone(),
        registration_ip: agent.registration_ip.clone(),
        last_ip: agent.last_ip.clone(),
        last_seen_at: agent.last_seen_at.clone(),
        internal_build_number: Some(agent.internal_build_number),
        stale_since: agent.stale_since.clone(),
        stale_reason: agent.stale_reason.clone(),
        extra: None,
    })
}

pub(crate) fn id_selector_expression(client_id: &str) -> String {
    vpsman_common::id_selector_expression(client_id)
}

#[cfg(test)]
#[path = "tests_selector_expression.rs"]
mod tests;
