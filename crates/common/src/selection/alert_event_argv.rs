use serde_json::Value;

use crate::{encode_json, payload_hash, JobCommand};

pub const ALERT_EVENT_ARGV_MAX_ELEMENTS: usize = 128;
pub const ALERT_EVENT_ARGV_MAX_ELEMENT_BYTES: usize = 16 * 1024;
pub const ALERT_EVENT_ARGV_MAX_BYTES: usize = 64 * 1024;
pub const ALERT_EVENT_NOOP_ARGV: &[&str] = &["/bin/true"];
pub const ALERT_EVENT_ARGV_CONTROL_TOKENS: &[&str] = &[
    "[if", "[elseif", "[else", "[for", "[end", "[endif", "[endfor",
];
pub const ALERT_EVENT_ARGV_HELPER_TOKENS: &[&str] = &[".filter", ".where", ".count", ".map"];
pub const ALERT_EVENT_ARGV_SCALAR_PATHS: &[&str] = &[
    "event.id",
    "event.kind",
    "event.occurred_at",
    "event.recorded_at",
    "alert.id",
    "alert.public_id",
    "alert.episode_id",
    "alert.title",
    "alert.detail",
    "alert.category",
    "alert.severity",
    "alert.record_kind",
    "alert.lifecycle_state",
    "alert.trigger_generation",
    "alert.source_status",
    "alert.resolution_reason",
    "alert.target_kind",
    "alert.target_id",
    "policy.id",
    "policy.name",
    "policy_rule.id",
    "policy_rule.name",
    "policy_rule.rule_version",
    "policy_rule.rule_kind",
    "policy_rule.evidence_source",
    "policy_rule.trigger_meta_condition.kind",
    "policy_rule.trigger_meta_condition.window_seconds",
    "policy_rule.resolve_meta_condition.kind",
    "policy_rule.resolve_meta_condition.window_seconds",
    "schedule.id",
    "schedule.name",
    "schedule.definition_revision",
    "schedule.fixed_target_count",
    "schedule.matched_subject_count",
];

/// Validates the stored alert-event argv template grammar without rendering it.
///
/// The grammar is intentionally smaller than the general message-template
/// grammar: literal text and direct allowlisted scalar `{path}` placeholders
/// only. A missing template denotes the fixed `/bin/true` no-op.
pub fn validate_alert_event_argv_template(template: Option<&[String]>) -> Result<(), String> {
    let argv = effective_template(template);
    if argv.is_empty() {
        return Err("event_argv_empty".to_string());
    }
    if argv.len() > ALERT_EVENT_ARGV_MAX_ELEMENTS {
        return Err("event_argv_too_many_elements".to_string());
    }
    if argv.iter().map(|value| value.len()).sum::<usize>() > ALERT_EVENT_ARGV_MAX_BYTES {
        return Err("event_argv_template_too_large".to_string());
    }
    for (index, value) in argv.iter().enumerate() {
        if value.is_empty() {
            return Err(format!("event_argv_element_empty_at_{index}"));
        }
        if value.contains('\0') {
            return Err(format!("event_argv_contains_nul_at_{index}"));
        }
        if value.len() > ALERT_EVENT_ARGV_MAX_ELEMENT_BYTES {
            return Err(format!("event_argv_element_too_large_at_{index}"));
        }
    }
    if argv[0].trim().is_empty() {
        return Err("event_argv0_empty".to_string());
    }
    if contains_template_syntax(argv[0]) {
        return Err("event_argv0_must_be_literal".to_string());
    }

    for (index, element) in argv.iter().enumerate().skip(1) {
        validate_direct_scalar_template(element)
            .map_err(|error| format!("event_argv_template_invalid_at_{index}: {error}"))?;
    }
    Ok(())
}

/// Returns whether the strict argv template references one exact scalar path.
///
/// Callers use this for shape-dependent fields such as
/// `alert.resolution_reason`, which is non-null only on Resolved edges.
pub fn alert_event_argv_template_uses_path(
    template: Option<&[String]>,
    expected_path: &str,
) -> Result<bool, String> {
    validate_alert_event_argv_template(template)?;
    let mut found = false;
    for element in effective_template(template) {
        visit_placeholders(element, |path| {
            found |= path == expected_path;
            Ok(())
        })?;
    }
    Ok(found)
}

/// Renders each stored argv element independently. Rendered whitespace is never
/// parsed or split into additional arguments.
pub fn render_alert_event_argv_template(
    template: Option<&[String]>,
    context: &Value,
) -> Result<Vec<String>, String> {
    validate_alert_event_argv_template(template)?;
    let argv = effective_template(template)
        .iter()
        .map(|element| render_direct_scalar_template(element, context))
        .collect::<Result<Vec<_>, _>>()?;
    validate_rendered_argv(&argv)?;
    Ok(argv)
}

/// Renders the shell job command and returns the hash of the exact canonical
/// operation persisted by the worker.
pub fn render_alert_event_job_command(
    template: Option<&[String]>,
    context: &Value,
) -> Result<(JobCommand, String), String> {
    let argv = render_alert_event_argv_template(template, context)?;
    let operation = JobCommand::Shell { argv, pty: false };
    let bytes = encode_json(&operation).map_err(|error| error.to_string())?;
    let hash = payload_hash(&bytes);
    Ok((operation, hash))
}

/// Hashes the canonical stored template intent. This is distinct from the
/// rendered operation hash returned by [`render_alert_event_job_command`].
pub fn alert_event_argv_template_hash(template: Option<&[String]>) -> Result<String, String> {
    validate_alert_event_argv_template(template)?;
    let owned = effective_template(template)
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    let bytes = encode_json(&owned).map_err(|error| error.to_string())?;
    Ok(payload_hash(&bytes))
}

fn effective_template(template: Option<&[String]>) -> Vec<&str> {
    template.map_or_else(
        || ALERT_EVENT_NOOP_ARGV.to_vec(),
        |argv| argv.iter().map(String::as_str).collect(),
    )
}

fn validate_direct_scalar_template(template: &str) -> Result<(), String> {
    if contains_control_or_helper_syntax(template) {
        return Err("event_argv_template_control_syntax_unsupported".to_string());
    }
    visit_placeholders(template, |_| Ok(()))
}

fn render_direct_scalar_template(template: &str, context: &Value) -> Result<String, String> {
    if contains_control_or_helper_syntax(template) {
        return Err("event_argv_template_control_syntax_unsupported".to_string());
    }
    let mut rendered = String::with_capacity(template.len());
    visit_template_parts(template, |part| match part {
        TemplatePart::Literal(value) => {
            rendered.push_str(value);
            Ok(())
        }
        TemplatePart::Placeholder(path) => {
            let value = path.split('.').try_fold(context, |value, segment| {
                value
                    .as_object()
                    .and_then(|object| object.get(segment))
                    .ok_or_else(|| format!("event argv scalar path {{{path}}} is missing"))
            })?;
            match value {
                Value::String(value) => rendered.push_str(value),
                Value::Bool(value) => rendered.push_str(if *value { "true" } else { "false" }),
                Value::Number(value) => rendered.push_str(&value.to_string()),
                Value::Null => {
                    return Err(format!(
                        "event argv scalar path {{{path}}} resolved to null"
                    ));
                }
                Value::Array(_) | Value::Object(_) => {
                    return Err(format!(
                        "event argv scalar path {{{path}}} did not resolve to a scalar"
                    ));
                }
            }
            Ok(())
        }
    })?;
    Ok(rendered)
}

fn validate_rendered_argv(argv: &[String]) -> Result<(), String> {
    if argv.is_empty() {
        return Err("event_argv_empty_after_render".to_string());
    }
    if argv[0].trim().is_empty() {
        return Err("event_argv0_empty_after_render".to_string());
    }
    if argv.len() > ALERT_EVENT_ARGV_MAX_ELEMENTS {
        return Err("event_argv_too_many_elements".to_string());
    }
    if argv.iter().map(|value| value.len()).sum::<usize>() > ALERT_EVENT_ARGV_MAX_BYTES {
        return Err("event_argv_too_large".to_string());
    }
    for (index, value) in argv.iter().enumerate() {
        if value.is_empty() {
            return Err(format!("event_argv_element_empty_after_render_at_{index}"));
        }
        if value.contains('\0') {
            return Err(format!("event_argv_contains_nul_at_{index}"));
        }
        if value.len() > ALERT_EVENT_ARGV_MAX_ELEMENT_BYTES {
            return Err(format!("event_argv_element_too_large_at_{index}"));
        }
    }
    Ok(())
}

fn visit_placeholders(
    template: &str,
    mut visit: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    visit_template_parts(template, |part| match part {
        TemplatePart::Literal(_) => Ok(()),
        TemplatePart::Placeholder(path) => visit(path),
    })
}

enum TemplatePart<'a> {
    Literal(&'a str),
    Placeholder(&'a str),
}

fn visit_template_parts(
    template: &str,
    mut visit: impl FnMut(TemplatePart<'_>) -> Result<(), String>,
) -> Result<(), String> {
    let mut remaining = template;
    loop {
        let Some(open) = remaining.find('{') else {
            if remaining.contains('}') {
                return Err("unexpected scalar placeholder closing brace".to_string());
            }
            visit(TemplatePart::Literal(remaining))?;
            return Ok(());
        };
        let literal = &remaining[..open];
        if literal.contains('}') {
            return Err("unexpected scalar placeholder closing brace".to_string());
        }
        visit(TemplatePart::Literal(literal))?;
        let after_open = &remaining[open + 1..];
        let close = after_open
            .find('}')
            .ok_or_else(|| "unclosed scalar placeholder".to_string())?;
        let path = &after_open[..close];
        if path.contains('{') {
            return Err("nested scalar placeholder".to_string());
        }
        if !ALERT_EVENT_ARGV_SCALAR_PATHS.contains(&path) {
            return Err(format!("unsupported event argv scalar path {{{path}}}"));
        }
        visit(TemplatePart::Placeholder(path))?;
        remaining = &after_open[close + 1..];
    }
}

fn contains_template_syntax(value: &str) -> bool {
    value.contains('{') || value.contains('}') || contains_control_or_helper_syntax(value)
}

fn contains_control_or_helper_syntax(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ALERT_EVENT_ARGV_CONTROL_TOKENS
        .iter()
        .chain(ALERT_EVENT_ARGV_HELPER_TOKENS)
        .any(|token| lower.contains(token))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn context() -> Value {
        json!({
            "event": {"kind": "alert.triggered", "id": "edge-1", "occurred_at": "2026-08-18T00:00:00Z", "recorded_at": "2026-08-18T00:00:01Z"},
            "alert": {
                "title": "Traffic threshold",
                "resolution_reason": null,
                "target_kind": "job",
                "target_id": "job-a"
            },
            "schedule": {"definition_revision": 4}
        })
    }

    #[test]
    fn strict_alert_event_argv_preserves_element_boundaries() {
        let template = vec![
            "/usr/bin/printf".to_string(),
            "%s\\n".to_string(),
            "{alert.title} ({alert.target_kind}:{alert.target_id})".to_string(),
            "{schedule.definition_revision}".to_string(),
        ];
        let (operation, hash) =
            render_alert_event_job_command(Some(&template), &context()).unwrap();
        let JobCommand::Shell { argv, pty } = operation else {
            panic!("expected shell operation");
        };
        assert!(!pty);
        assert_eq!(
            argv,
            [
                "/usr/bin/printf",
                "%s\\n",
                "Traffic threshold (job:job-a)",
                "4"
            ]
        );
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn strict_alert_event_argv_rejects_dynamic_programs_and_non_scalars() {
        assert!(validate_alert_event_argv_template(Some(&["{alert.title}".to_string()])).is_err());
        assert!(render_alert_event_argv_template(
            Some(&[
                "/bin/echo".to_string(),
                "{alert.resolution_reason}".to_string()
            ]),
            &context(),
        )
        .is_err());
        assert!(validate_alert_event_argv_template(Some(&[
            "/bin/echo".to_string(),
            "[if alert.triggered]yes[endif]".to_string(),
        ]))
        .is_err());
        assert!(validate_alert_event_argv_template(Some(&[
            "/bin/echo".to_string(),
            String::new(),
        ]))
        .is_err());
        let mut empty_context = context();
        empty_context["alert"]["title"] = Value::String(String::new());
        assert!(render_alert_event_argv_template(
            Some(&["/bin/echo".to_string(), "{alert.title}".to_string()]),
            &empty_context,
        )
        .is_err());
    }

    #[test]
    fn strict_alert_event_argv_does_not_infer_executable_semantics() {
        for (template, expected_last) in [
            (
                vec!["/bin/sh", "-c", "echo {alert.title}"],
                "echo Traffic threshold",
            ),
            (
                vec!["/usr/bin/env", "--split-string=bash -c echo {alert.title}"],
                "--split-string=bash -c echo Traffic threshold",
            ),
            (
                vec!["sudo", "busybox", "sh", "-c", "echo {alert.title}"],
                "echo Traffic threshold",
            ),
        ] {
            let template = template.into_iter().map(str::to_string).collect::<Vec<_>>();
            let rendered = render_alert_event_argv_template(Some(&template), &context()).unwrap();
            assert_eq!(rendered.len(), template.len());
            assert_eq!(rendered.last().unwrap(), expected_last);
        }
    }

    #[test]
    fn missing_template_is_a_hashed_noop() {
        assert_eq!(
            render_alert_event_argv_template(None, &context()).unwrap(),
            ["/bin/true"]
        );
        assert_eq!(alert_event_argv_template_hash(None).unwrap().len(), 64);
    }

    #[test]
    fn immutable_expression_fields_are_all_valid_scalar_argv_paths() {
        for field in crate::ALERT_EVENT_IMMUTABLE_FIELDS {
            if matches!(*field, "alert.client_id" | "policy_rule.system_seed_key") {
                continue;
            }
            assert!(
                ALERT_EVENT_ARGV_SCALAR_PATHS.contains(field),
                "missing argv scalar path for immutable event field {field}"
            );
        }
    }

    #[test]
    fn strict_alert_event_argv_reports_exact_path_usage() {
        let template = vec![
            "/bin/echo".to_string(),
            "{alert.resolution_reason}: {alert.title}".to_string(),
        ];
        assert!(
            alert_event_argv_template_uses_path(Some(&template), "alert.resolution_reason")
                .unwrap()
        );
        assert!(!alert_event_argv_template_uses_path(Some(&template), "alert.detail").unwrap());
    }
}
