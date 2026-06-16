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
mod tests {
    use super::*;
    use vpsman_common::AgentCapabilitySnapshot;

    fn agent(id: &str, name: &str, status: &str, tags: &[&str]) -> AgentView {
        AgentView {
            id: id.to_string(),
            display_name: name.to_string(),
            status: status.to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: Some("2026-06-08T01:00:00Z".to_string()),
            internal_build_number: 42,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        }
    }

    #[test]
    fn parses_boolean_expression_with_implicit_and_and_aliases() {
        let expression =
            parse_selector_expression("(provider:alpha || provider:beta) country:US id:edge-*")
                .unwrap()
                .unwrap();
        assert!(agent_matches_selector_expression(
            &agent(
                "edge-01",
                "Edge One",
                "online",
                &["provider:alpha", "country:US"]
            ),
            &expression,
        ));
        assert!(!agent_matches_selector_expression(
            &agent(
                "edge-01",
                "Edge One",
                "online",
                &["provider:gamma", "country:US"]
            ),
            &expression,
        ));
    }

    #[test]
    fn supports_comparisons_membership_not_and_untagged() {
        let expression = parse_selector_expression(
            r#"status = online && vps.tag in [edge, /^prod-.*/] && !untagged && last_seen < 2026-06-08T02:00:00Z && vps.internal_build_number > 10"#,
        )
        .unwrap()
        .unwrap();
        assert!(agent_matches_selector_expression(
            &agent(
                "agent-sfo-01",
                "edge-sfo-01",
                "online",
                &["edge", "provider:alpha", "country:US"]
            ),
            &expression,
        ));
    }

    #[test]
    fn unknown_namespaced_terms_are_exact_tag_names() {
        let expression = parse_selector_expression("role:edge").unwrap().unwrap();
        assert!(agent_matches_selector_expression(
            &agent("edge-01", "Edge One", "online", &["role:edge"]),
            &expression,
        ));
        assert!(!agent_matches_selector_expression(
            &agent("edge-01", "Edge One", "online", &["edge"]),
            &expression,
        ));
    }

    #[test]
    fn invalid_expressions_report_errors() {
        assert!(parse_selector_expression("(provider:alpha").is_err());
        assert!(parse_selector_expression("provider:").is_err());
        assert!(parse_selector_expression("status in []").is_err());
    }
}
