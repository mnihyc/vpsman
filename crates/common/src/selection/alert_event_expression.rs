use crate::{parse_expression, Expression, Predicate};

pub const ALERT_EVENT_IMMUTABLE_FIELDS: &[&str] = &[
    "event.id",
    "event.kind",
    "event.occurred_at",
    "event.recorded_at",
    "alert.id",
    "alert.public_id",
    "alert.episode_id",
    "alert.record_kind",
    "alert.category",
    "alert.severity",
    "alert.lifecycle_state",
    "alert.trigger_generation",
    "alert.source_status",
    "alert.resolution_reason",
    "alert.title",
    "alert.detail",
    "alert.client_id",
    "alert.target_kind",
    "alert.target_id",
    "policy.id",
    "policy.name",
    "policy_rule.id",
    "policy_rule.name",
    "policy_rule.rule_version",
    "policy_rule.rule_kind",
    "policy_rule.evidence_source",
    "policy_rule.system_seed_key",
    "policy_rule.trigger_meta_condition.kind",
    "policy_rule.trigger_meta_condition.window_seconds",
    "policy_rule.resolve_meta_condition.kind",
    "policy_rule.resolve_meta_condition.window_seconds",
];

pub const ALERT_EVENT_CATEGORIES: &[&str] = &[
    "agent_status",
    "network",
    "backup",
    "agent_update",
    "job",
    "capability_degraded",
    "traffic",
    "resource",
];
pub const ALERT_EVENT_SEVERITIES: &[&str] = &["info", "warning", "critical"];

pub fn parse_and_validate_alert_event_expression(input: &str) -> Result<Expression, String> {
    let expression =
        parse_expression(input)?.ok_or_else(|| "event expression is empty".to_string())?;
    validate_alert_event_expression(&expression)?;
    Ok(expression)
}

pub fn validate_alert_event_expression(expression: &Expression) -> Result<(), String> {
    if contains_unsupported_alert_predicate(expression) {
        return Err("event_expression_not_alert_only".to_string());
    }
    if !every_or_branch_has_lifecycle_anchor(expression) {
        return Err("event_expression_missing_lifecycle_anchor".to_string());
    }
    Ok(())
}

/// Returns the canonical lifecycle edge kinds positively anchoring this
/// validated schedule expression as `(triggered, resolved)`.
///
/// The result is intentionally conservative: an impossible conjunction such
/// as `alert.triggered && alert.resolved` reports both kinds. This means a
/// caller validating a strict argv template will check every edge shape that
/// the author named and can never accidentally validate only one side of a
/// paired expression.
pub fn alert_event_expression_anchor_kinds(expression: &Expression) -> (bool, bool) {
    fn visit(expression: &Expression, triggered: &mut bool, resolved: &mut bool) {
        match expression {
            Expression::Predicate(Predicate::Event(event)) => match event.as_str() {
                "alert.triggered" => *triggered = true,
                "alert.resolved" => *resolved = true,
                _ => {}
            },
            Expression::Not(_) | Expression::Predicate(_) => {}
            Expression::And(left, right) | Expression::Or(left, right) => {
                visit(left, triggered, resolved);
                visit(right, triggered, resolved);
            }
        }
    }

    let mut triggered = false;
    let mut resolved = false;
    visit(expression, &mut triggered, &mut resolved);
    (triggered, resolved)
}

fn contains_unsupported_alert_predicate(expression: &Expression) -> bool {
    match expression {
        Expression::Predicate(Predicate::Bare(_) | Predicate::Untagged) => true,
        Expression::Predicate(Predicate::Event(event)) => {
            let event = event.to_ascii_lowercase();
            !(matches!(event.as_str(), "alert.triggered" | "alert.resolved")
                || event
                    .strip_prefix("alert.category:")
                    .is_some_and(|value| ALERT_EVENT_CATEGORIES.contains(&value))
                || event
                    .strip_prefix("alert.severity:")
                    .is_some_and(|value| ALERT_EVENT_SEVERITIES.contains(&value)))
        }
        Expression::Predicate(Predicate::Comparison { field, .. })
        | Expression::Predicate(Predicate::Membership { field, .. }) => {
            !ALERT_EVENT_IMMUTABLE_FIELDS
                .iter()
                .any(|allowed| field.eq_ignore_ascii_case(allowed))
        }
        Expression::Not(inner) => contains_unsupported_alert_predicate(inner),
        Expression::And(left, right) | Expression::Or(left, right) => {
            contains_unsupported_alert_predicate(left)
                || contains_unsupported_alert_predicate(right)
        }
    }
}

fn every_or_branch_has_lifecycle_anchor(expression: &Expression) -> bool {
    match expression {
        Expression::Predicate(Predicate::Event(event)) => {
            matches!(event.as_str(), "alert.triggered" | "alert.resolved")
        }
        Expression::Predicate(_) | Expression::Not(_) => false,
        Expression::And(left, right) => {
            every_or_branch_has_lifecycle_anchor(left)
                || every_or_branch_has_lifecycle_anchor(right)
        }
        Expression::Or(left, right) => {
            every_or_branch_has_lifecycle_anchor(left)
                && every_or_branch_has_lifecycle_anchor(right)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_event_expression_requires_a_canonical_anchor_per_or_branch() {
        assert!(parse_and_validate_alert_event_expression(
            "alert.triggered && alert.category:traffic"
        )
        .is_ok());
        assert!(parse_and_validate_alert_event_expression(
            "(alert.triggered && alert.severity:critical) || (alert.resolved && policy.name = Traffic)"
        )
        .is_ok());
        assert!(parse_and_validate_alert_event_expression(
            "alert.triggered || telemetry.network_rate"
        )
        .is_err());
        assert!(parse_and_validate_alert_event_expression(
            "!alert.triggered && alert.category:traffic"
        )
        .is_err());
        assert!(parse_and_validate_alert_event_expression("alert.open").is_err());
        assert!(
            parse_and_validate_alert_event_expression("alert.triggered && alert.state:open")
                .is_err()
        );
        assert!(parse_and_validate_alert_event_expression(
            "alert.triggered && alert.severity:critcal"
        )
        .is_err());
        assert!(parse_and_validate_alert_event_expression(
            "alert.triggered && alert.category:trafic"
        )
        .is_err());
    }

    #[test]
    fn alert_event_expression_reports_every_positive_edge_anchor() {
        let expression = parse_and_validate_alert_event_expression(
            "(alert.triggered && alert.severity:critical) || (alert.resolved && !alert.triggered)",
        )
        .unwrap();
        assert_eq!(
            alert_event_expression_anchor_kinds(&expression),
            (true, true)
        );

        let expression = parse_and_validate_alert_event_expression(
            "ALERT.RESOLVED && event.kind = alert.resolved",
        )
        .unwrap();
        assert_eq!(
            alert_event_expression_anchor_kinds(&expression),
            (false, true)
        );
    }
}
