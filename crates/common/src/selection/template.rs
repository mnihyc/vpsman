use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde_json::{json, Map, Value};

use crate::{
    canonical_retired_policy_rule_path, expression_matches, parse_expression,
    rewrite_retired_alert_event_aliases, ExpressionContext, VpsMetadata,
};

const DEFAULT_MESSAGE_LIMIT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateError {
    pub errors: Vec<String>,
}

#[derive(Clone, Debug)]
enum Node {
    Text(String),
    Placeholder(PathExpr),
    For {
        variable: String,
        path: PathExpr,
        body: Vec<Node>,
    },
    If {
        branches: Vec<(String, Vec<Node>)>,
        else_body: Vec<Node>,
    },
}

#[derive(Clone, Debug)]
struct PathExpr {
    base: String,
    helpers: Vec<HelperCall>,
}

#[derive(Clone, Debug)]
enum HelperCall {
    Length,
    Join(String),
    First,
    Last,
    Map(String),
    Filter(String),
    Count(Option<String>),
}

#[derive(Clone, Debug)]
struct Scope<'a> {
    root: &'a Value,
    locals: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EndTag {
    EndFor,
    ElseIf(String),
    Else,
    EndIf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConditionAliasMode {
    Reject,
    AcceptForRewrite,
}

impl TemplateError {
    fn single(error: impl Into<String>) -> Self {
        Self {
            errors: vec![error.into()],
        }
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.errors.join("; "))
    }
}

impl std::error::Error for TemplateError {}

pub fn validate_template(template: &str) -> Result<(), TemplateError> {
    parse_template(template).map(|_| ())
}

/// Rewrites retired alert lifecycle aliases and policy-rule paths in template
/// conditions, condition helpers, placeholders, map helpers, and for-loop paths
/// while preserving unrelated template source text.
pub fn rewrite_template_retired_alert_event_aliases(
    template: &str,
) -> Result<String, TemplateError> {
    parse_template_with_alias_mode(template, ConditionAliasMode::AcceptForRewrite)?;
    let rewritten = rewrite_template_condition_tags(template)?;
    parse_template(&rewritten)?;
    Ok(rewritten)
}

pub fn render_template(template: &str, context: &Value) -> Result<String, TemplateError> {
    render_template_with_limit(template, context, DEFAULT_MESSAGE_LIMIT_BYTES)
}

/// Renders a template whose interpolated values must resolve to scalars.
///
/// Missing values, arrays, objects, and `[for]` blocks are rejected. Helpers
/// that reduce a collection to a scalar, such as `.length`, `.count`, or
/// `.join(...)`, remain available.
pub fn render_scalar_template(template: &str, context: &Value) -> Result<String, TemplateError> {
    let nodes = parse_template(template)?;
    validate_scalar_template_nodes(&nodes)?;
    let scope = Scope {
        root: context,
        locals: BTreeMap::new(),
    };
    let rendered = render_scalar_nodes(&nodes, &scope)?;
    if rendered.len() > DEFAULT_MESSAGE_LIMIT_BYTES {
        return Err(TemplateError::single(
            "rendered message exceeds length limit",
        ));
    }
    Ok(rendered)
}

pub fn render_template_with_limit(
    template: &str,
    context: &Value,
    max_message_bytes: usize,
) -> Result<String, TemplateError> {
    let nodes = parse_template(template)?;
    let scope = Scope {
        root: context,
        locals: BTreeMap::new(),
    };
    let rendered = render_nodes(&nodes, &scope)?;
    if rendered.len() > max_message_bytes {
        return Err(TemplateError::single(
            "rendered message exceeds length limit",
        ));
    }
    Ok(rendered)
}

pub fn default_webhook_message(rule_name: &str, matched_vps_count: usize) -> String {
    format!(
        "{rule_name} matched {matched_vps_count} VPS{}",
        if matched_vps_count == 1 { "" } else { "s" }
    )
}

fn parse_template(input: &str) -> Result<Vec<Node>, TemplateError> {
    parse_template_with_alias_mode(input, ConditionAliasMode::Reject)
}

fn parse_template_with_alias_mode(
    input: &str,
    condition_alias_mode: ConditionAliasMode,
) -> Result<Vec<Node>, TemplateError> {
    let mut cursor = 0_usize;
    let (nodes, end_tag) = parse_nodes(input, &mut cursor, false, condition_alias_mode)?;
    if let Some(end_tag) = end_tag {
        return Err(TemplateError::single(format!(
            "unexpected closing block tag {}",
            end_tag_label(&end_tag)
        )));
    }
    Ok(nodes)
}

fn rewrite_template_condition_tags(input: &str) -> Result<String, TemplateError> {
    let mut rewritten = String::with_capacity(input.len());
    let mut cursor = 0_usize;
    while cursor < input.len() {
        let remainder = &input[cursor..];
        let placeholder_offset = remainder.find('{');
        let block_offset = remainder.find('[');
        let next_offset = match (placeholder_offset, block_offset) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        };
        let Some(offset) = next_offset else {
            rewritten.push_str(remainder);
            break;
        };
        let start = cursor + offset;
        rewritten.push_str(&input[cursor..start]);

        if input[start..].starts_with("{#") {
            let Some(end_offset) = input[start + 2..].find("#}") else {
                return Err(TemplateError::single("unmatched comment"));
            };
            let end = start + end_offset + 4;
            rewritten.push_str(&input[start..end]);
            cursor = end;
            continue;
        }
        if input[start..].starts_with('{') {
            let Some(end_offset) = input[start + 1..].find('}') else {
                return Err(TemplateError::single("unmatched placeholder"));
            };
            let end = start + end_offset + 2;
            let raw = &input[start + 1..end - 1];
            let canonical = rewrite_template_path_expression(raw)?;
            if canonical == raw {
                rewritten.push_str(&input[start..end]);
            } else {
                rewritten.push('{');
                rewritten.push_str(&canonical);
                rewritten.push('}');
            }
            cursor = end;
            continue;
        }

        let Some(end_offset) = input[start + 1..].find(']') else {
            rewritten.push_str(&input[start..]);
            break;
        };
        let end = start + end_offset + 2;
        let tag = input[start + 1..end - 1].trim();
        let condition_tag = tag
            .strip_prefix("if ")
            .map(|condition| ("if", condition.trim()))
            .or_else(|| {
                tag.strip_prefix("elseif ")
                    .map(|condition| ("elseif", condition.trim()))
            });
        if let Some((label, condition)) = condition_tag {
            let canonical = rewrite_retired_alert_event_aliases(condition).map_err(|error| {
                TemplateError::single(format!("invalid condition expression: {error}"))
            })?;
            if canonical == condition {
                rewritten.push_str(&input[start..end]);
            } else {
                rewritten.push_str(&format!("[{label} {canonical}]"));
            }
        } else if let Some(rest) = tag.strip_prefix("for ") {
            let (variable, path) = rest
                .split_once(" in ")
                .expect("validated for tag must contain ` in `");
            let path = path.trim();
            let canonical = rewrite_template_path_expression(path)?;
            if canonical == path {
                rewritten.push_str(&input[start..end]);
            } else {
                rewritten.push_str(&format!("[for {} in {canonical}]", variable.trim()));
            }
        } else {
            rewritten.push_str(&input[start..end]);
        }
        cursor = end;
    }
    Ok(rewritten)
}

fn parse_nodes(
    input: &str,
    cursor: &mut usize,
    in_block: bool,
    condition_alias_mode: ConditionAliasMode,
) -> Result<(Vec<Node>, Option<EndTag>), TemplateError> {
    let mut nodes = Vec::new();
    while *cursor < input.len() {
        let remainder = &input[*cursor..];
        let placeholder_offset = remainder.find('{');
        let block_offset = remainder.find('[');
        let next_offset = match (placeholder_offset, block_offset) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        };
        let Some(offset) = next_offset else {
            nodes.push(Node::Text(remainder.to_string()));
            *cursor = input.len();
            break;
        };
        if offset > 0 {
            nodes.push(Node::Text(remainder[..offset].to_string()));
            *cursor += offset;
        }
        if input[*cursor..].starts_with("{#") {
            let comment_start = *cursor;
            let Some(end) = input[*cursor + 2..].find("#}") else {
                return Err(TemplateError::single("unmatched comment"));
            };
            let comment_end = *cursor + end + 4;
            if let Some(next_cursor) = standalone_comment_end(input, comment_start, comment_end) {
                remove_comment_line_indentation(&mut nodes, input, comment_start);
                *cursor = next_cursor;
            } else {
                *cursor = comment_end;
            }
            continue;
        }
        if input[*cursor..].starts_with('{') {
            if let Some(end) = input[*cursor + 1..].find('}') {
                let raw = input[*cursor + 1..*cursor + 1 + end].trim();
                if raw.is_empty() {
                    return Err(TemplateError::single("empty placeholder"));
                }
                nodes.push(Node::Placeholder(parse_path_expr_with_alias_mode(
                    raw,
                    condition_alias_mode,
                )?));
                *cursor += end + 2;
            } else {
                return Err(TemplateError::single("unmatched placeholder"));
            }
            continue;
        }
        let Some(end) = input[*cursor + 1..].find(']') else {
            nodes.push(Node::Text("[".to_string()));
            *cursor += 1;
            continue;
        };
        let tag = input[*cursor + 1..*cursor + 1 + end].trim();
        let tag_len = end + 2;
        if let Some(end_tag) = parse_end_tag(tag) {
            if in_block {
                *cursor += tag_len;
                return Ok((nodes, Some(end_tag)));
            }
            return Err(TemplateError::single(format!(
                "unexpected closing block tag {}",
                end_tag_label(&end_tag)
            )));
        }
        if let Some((variable, path)) = parse_for_tag(tag, condition_alias_mode)? {
            *cursor += tag_len;
            let (body, end_tag) = parse_nodes(input, cursor, true, condition_alias_mode)?;
            match end_tag {
                Some(EndTag::EndFor) => nodes.push(Node::For {
                    variable,
                    path,
                    body,
                }),
                Some(other) => {
                    return Err(TemplateError::single(format!(
                        "for block closed by {}",
                        end_tag_label(&other)
                    )));
                }
                None => return Err(TemplateError::single("unmatched for block")),
            }
            continue;
        }
        if let Some(condition) = parse_if_tag(tag, condition_alias_mode)? {
            *cursor += tag_len;
            let mut branches = Vec::new();
            let (body, mut end_tag) = parse_nodes(input, cursor, true, condition_alias_mode)?;
            branches.push((condition, body));
            let mut else_body = Vec::new();
            loop {
                match end_tag {
                    Some(EndTag::ElseIf(condition)) => {
                        validate_condition_with_alias_mode(&condition, condition_alias_mode)?;
                        let (body, next) = parse_nodes(input, cursor, true, condition_alias_mode)?;
                        branches.push((condition, body));
                        end_tag = next;
                    }
                    Some(EndTag::Else) => {
                        let (body, next) = parse_nodes(input, cursor, true, condition_alias_mode)?;
                        else_body = body;
                        match next {
                            Some(EndTag::EndIf) => break,
                            Some(other) => {
                                return Err(TemplateError::single(format!(
                                    "else block closed by {}",
                                    end_tag_label(&other)
                                )));
                            }
                            None => return Err(TemplateError::single("unmatched if block")),
                        }
                    }
                    Some(EndTag::EndIf) => break,
                    Some(other) => {
                        return Err(TemplateError::single(format!(
                            "if block closed by {}",
                            end_tag_label(&other)
                        )));
                    }
                    None => return Err(TemplateError::single("unmatched if block")),
                }
            }
            nodes.push(Node::If {
                branches,
                else_body,
            });
            continue;
        }
        nodes.push(Node::Text(input[*cursor..*cursor + tag_len].to_string()));
        *cursor += tag_len;
    }
    Ok((nodes, None))
}

fn standalone_comment_end(input: &str, comment_start: usize, comment_end: usize) -> Option<usize> {
    let line_start = input[..comment_start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    if !input[line_start..comment_start].trim().is_empty() {
        return None;
    }
    let remaining = &input[comment_end..];
    let line_end_offset = remaining.find('\n').unwrap_or(remaining.len());
    if !remaining[..line_end_offset].trim().is_empty() {
        return None;
    }
    Some(if line_end_offset < remaining.len() {
        comment_end + line_end_offset + 1
    } else {
        input.len()
    })
}

fn remove_comment_line_indentation(nodes: &mut [Node], input: &str, comment_start: usize) {
    let line_start = input[..comment_start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let indentation = &input[line_start..comment_start];
    if indentation.is_empty() {
        return;
    }
    let Some(Node::Text(text)) = nodes.last_mut() else {
        return;
    };
    if text.ends_with(indentation) {
        text.truncate(text.len() - indentation.len());
    }
}

fn parse_end_tag(tag: &str) -> Option<EndTag> {
    if tag == "endfor" {
        Some(EndTag::EndFor)
    } else if tag == "else" {
        Some(EndTag::Else)
    } else if tag == "endif" {
        Some(EndTag::EndIf)
    } else {
        tag.strip_prefix("elseif ")
            .map(str::trim)
            .filter(|condition| !condition.is_empty())
            .map(|condition| EndTag::ElseIf(condition.to_string()))
    }
}

fn parse_for_tag(
    tag: &str,
    condition_alias_mode: ConditionAliasMode,
) -> Result<Option<(String, PathExpr)>, TemplateError> {
    let Some(rest) = tag.strip_prefix("for ") else {
        return Ok(None);
    };
    let Some((variable, path)) = rest.split_once(" in ") else {
        return Err(TemplateError::single("invalid for block syntax"));
    };
    let variable = variable.trim();
    if !is_identifier(variable) {
        return Err(TemplateError::single("invalid loop variable"));
    }
    let path = path.trim();
    if path.is_empty() {
        return Err(TemplateError::single(
            "for block is missing an iterable path",
        ));
    }
    Ok(Some((
        variable.to_string(),
        parse_path_expr_with_alias_mode(path, condition_alias_mode)?,
    )))
}

fn parse_if_tag(
    tag: &str,
    condition_alias_mode: ConditionAliasMode,
) -> Result<Option<String>, TemplateError> {
    let Some(condition) = tag.strip_prefix("if ") else {
        return Ok(None);
    };
    let condition = condition.trim();
    if condition.is_empty() {
        return Err(TemplateError::single("if block is missing a condition"));
    }
    validate_condition_with_alias_mode(condition, condition_alias_mode)?;
    Ok(Some(condition.to_string()))
}

fn validate_condition(condition: &str) -> Result<(), TemplateError> {
    parse_expression(condition)
        .map_err(|error| TemplateError::single(format!("invalid condition expression: {error}")))?;
    Ok(())
}

fn validate_condition_with_alias_mode(
    condition: &str,
    condition_alias_mode: ConditionAliasMode,
) -> Result<(), TemplateError> {
    match condition_alias_mode {
        ConditionAliasMode::Reject => validate_condition(condition),
        ConditionAliasMode::AcceptForRewrite => rewrite_retired_alert_event_aliases(condition)
            .map(|_| ())
            .map_err(|error| {
                TemplateError::single(format!("invalid condition expression: {error}"))
            }),
    }
}

fn parse_path_expr_with_alias_mode(
    raw: &str,
    condition_alias_mode: ConditionAliasMode,
) -> Result<PathExpr, TemplateError> {
    let helper_start = first_helper_start(raw).unwrap_or(raw.len());
    let base = raw[..helper_start].trim();
    if base.is_empty() {
        return Err(TemplateError::single("path is missing a root"));
    }
    let mut helpers = Vec::new();
    let mut cursor = helper_start;
    while cursor < raw.len() {
        let tail = &raw[cursor..];
        if tail.starts_with(".length") {
            helpers.push(HelperCall::Length);
            cursor += ".length".len();
        } else if tail.starts_with(".first") {
            helpers.push(HelperCall::First);
            cursor += ".first".len();
        } else if tail.starts_with(".last") {
            helpers.push(HelperCall::Last);
            cursor += ".last".len();
        } else if tail.starts_with(".join(") {
            let (argument, next) = helper_argument(raw, cursor + ".join".len())?;
            helpers.push(HelperCall::Join(unquote(argument.trim())));
            cursor = next;
        } else if tail.starts_with(".map(") {
            let (argument, next) = helper_argument(raw, cursor + ".map".len())?;
            let argument = argument.trim();
            if argument.is_empty() || first_helper_start(argument).is_some() {
                return Err(TemplateError::single("invalid map helper syntax"));
            }
            helpers.push(HelperCall::Map(argument.to_string()));
            cursor = next;
        } else if tail.starts_with(".filter(") || tail.starts_with(".where(") {
            let helper_name_len = if tail.starts_with(".filter(") {
                ".filter".len()
            } else {
                ".where".len()
            };
            let (argument, next) = helper_argument(raw, cursor + helper_name_len)?;
            let argument = argument.trim();
            if argument.is_empty() {
                return Err(TemplateError::single(
                    "filter helper is missing a condition",
                ));
            }
            validate_condition_with_alias_mode(argument, condition_alias_mode)?;
            helpers.push(HelperCall::Filter(argument.to_string()));
            cursor = next;
        } else if tail.starts_with(".count(") {
            let (argument, next) = helper_argument(raw, cursor + ".count".len())?;
            let argument = argument.trim();
            if argument.is_empty() {
                helpers.push(HelperCall::Count(None));
            } else {
                validate_condition_with_alias_mode(argument, condition_alias_mode)?;
                helpers.push(HelperCall::Count(Some(argument.to_string())));
            }
            cursor = next;
        } else if tail.starts_with(".count") {
            helpers.push(HelperCall::Count(None));
            cursor += ".count".len();
        } else {
            return Err(TemplateError::single(format!(
                "invalid helper syntax near {tail}"
            )));
        }
    }
    Ok(PathExpr {
        base: base.to_string(),
        helpers,
    })
}

fn rewrite_template_path_expression(raw: &str) -> Result<String, TemplateError> {
    parse_path_expr_with_alias_mode(raw, ConditionAliasMode::AcceptForRewrite)?;

    let helper_start = first_helper_start(raw).unwrap_or(raw.len());
    let mut rewritten = rewrite_exact_policy_rule_path_source(&raw[..helper_start]);
    let mut cursor = helper_start;
    while cursor < raw.len() {
        let tail = &raw[cursor..];
        if tail.starts_with(".length") {
            rewritten.push_str(".length");
            cursor += ".length".len();
        } else if tail.starts_with(".first") {
            rewritten.push_str(".first");
            cursor += ".first".len();
        } else if tail.starts_with(".last") {
            rewritten.push_str(".last");
            cursor += ".last".len();
        } else if tail.starts_with(".join(") {
            let (_argument, next) = helper_argument(raw, cursor + ".join".len())?;
            rewritten.push_str(&raw[cursor..next]);
            cursor = next;
        } else if tail.starts_with(".map(") {
            let (argument, next) = helper_argument(raw, cursor + ".map".len())?;
            let argument_start = cursor + ".map(".len();
            rewritten.push_str(&raw[cursor..argument_start]);
            rewritten.push_str(&rewrite_exact_policy_rule_path_source(argument));
            rewritten.push(')');
            cursor = next;
        } else if tail.starts_with(".filter(") || tail.starts_with(".where(") {
            let helper_name_len = if tail.starts_with(".filter(") {
                ".filter".len()
            } else {
                ".where".len()
            };
            let (argument, next) = helper_argument(raw, cursor + helper_name_len)?;
            let argument_start = cursor + helper_name_len + 1;
            rewritten.push_str(&raw[cursor..argument_start]);
            rewritten.push_str(&rewrite_template_condition_source(argument)?);
            rewritten.push(')');
            cursor = next;
        } else if tail.starts_with(".count(") {
            let (argument, next) = helper_argument(raw, cursor + ".count".len())?;
            let argument_start = cursor + ".count(".len();
            rewritten.push_str(&raw[cursor..argument_start]);
            if argument.trim().is_empty() {
                rewritten.push_str(argument);
            } else {
                rewritten.push_str(&rewrite_template_condition_source(argument)?);
            }
            rewritten.push(')');
            cursor = next;
        } else if tail.starts_with(".count") {
            rewritten.push_str(".count");
            cursor += ".count".len();
        } else {
            return Err(TemplateError::single(format!(
                "invalid helper syntax near {tail}"
            )));
        }
    }
    Ok(rewritten)
}

fn rewrite_template_condition_source(source: &str) -> Result<String, TemplateError> {
    let condition = source.trim();
    let canonical = rewrite_retired_alert_event_aliases(condition)
        .map_err(|error| TemplateError::single(format!("invalid condition expression: {error}")))?;
    Ok(replace_trimmed_source(source, condition, &canonical))
}

fn rewrite_exact_policy_rule_path_source(source: &str) -> String {
    let path = source.trim();
    let Some(canonical) = canonical_retired_policy_rule_path(path) else {
        return source.to_string();
    };
    replace_trimmed_source(source, path, canonical)
}

fn replace_trimmed_source(source: &str, trimmed: &str, replacement: &str) -> String {
    if trimmed == replacement {
        return source.to_string();
    }
    let leading_len = source.len() - source.trim_start().len();
    let trailing_start = source.trim_end().len();
    format!(
        "{}{}{}",
        &source[..leading_len],
        replacement,
        &source[trailing_start..]
    )
}

fn first_helper_start(raw: &str) -> Option<usize> {
    [
        ".length", ".join(", ".first", ".last", ".map(", ".filter(", ".where(", ".count(", ".count",
    ]
    .iter()
    .filter_map(|needle| raw.find(needle))
    .min()
}

fn helper_argument(raw: &str, open_paren_index: usize) -> Result<(&str, usize), TemplateError> {
    if raw.as_bytes().get(open_paren_index) != Some(&b'(') {
        return Err(TemplateError::single(
            "helper is missing opening parenthesis",
        ));
    }
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (offset, character) in raw[open_paren_index..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
            continue;
        }
        if character == '"' || character == '\'' {
            quote = Some(character);
            continue;
        }
        if character == '(' {
            depth += 1;
        } else if character == ')' {
            depth -= 1;
            if depth == 0 {
                let close = open_paren_index + offset;
                return Ok((&raw[open_paren_index + 1..close], close + 1));
            }
        }
    }
    Err(TemplateError::single(
        "helper is missing closing parenthesis",
    ))
}

fn render_nodes(nodes: &[Node], scope: &Scope<'_>) -> Result<String, TemplateError> {
    let mut output = String::new();
    for node in nodes {
        match node {
            Node::Text(text) => output.push_str(text),
            Node::Placeholder(path) => {
                output.push_str(&render_value(&resolve_path_expr(path, scope)?))
            }
            Node::For {
                variable,
                path,
                body,
            } => {
                let iterable = resolve_path_expr(path, scope)?;
                if let Value::Array(values) = iterable {
                    for value in values {
                        let mut child = scope.clone();
                        child.locals.insert(variable.clone(), value);
                        output.push_str(&render_nodes(body, &child)?);
                    }
                }
            }
            Node::If {
                branches,
                else_body,
            } => {
                let mut rendered = false;
                for (condition, body) in branches {
                    if condition_matches(condition, scope, None)? {
                        output.push_str(&render_nodes(body, scope)?);
                        rendered = true;
                        break;
                    }
                }
                if !rendered {
                    output.push_str(&render_nodes(else_body, scope)?);
                }
            }
        }
    }
    Ok(output)
}

fn validate_scalar_template_nodes(nodes: &[Node]) -> Result<(), TemplateError> {
    for node in nodes {
        match node {
            Node::For { .. } => {
                return Err(TemplateError::single(
                    "scalar templates do not support for blocks",
                ));
            }
            Node::If {
                branches,
                else_body,
            } => {
                for (_condition, body) in branches {
                    validate_scalar_template_nodes(body)?;
                }
                validate_scalar_template_nodes(else_body)?;
            }
            Node::Text(_) | Node::Placeholder(_) => {}
        }
    }
    Ok(())
}

fn render_scalar_nodes(nodes: &[Node], scope: &Scope<'_>) -> Result<String, TemplateError> {
    let mut output = String::new();
    for node in nodes {
        match node {
            Node::Text(text) => output.push_str(text),
            Node::Placeholder(path) => {
                let value = resolve_path_expr(path, scope)?;
                match value {
                    Value::Null => {
                        return Err(TemplateError::single(format!(
                            "template scalar path `{}` is missing",
                            path.base
                        )));
                    }
                    Value::Array(_) | Value::Object(_) => {
                        return Err(TemplateError::single(format!(
                            "template path `{}` did not resolve to a scalar",
                            path.base
                        )));
                    }
                    Value::String(value) => output.push_str(&value),
                    Value::Number(value) => output.push_str(&value.to_string()),
                    Value::Bool(value) => output.push_str(if value { "true" } else { "false" }),
                }
            }
            Node::For { .. } => unreachable!("scalar template nodes were validated"),
            Node::If {
                branches,
                else_body,
            } => {
                let mut rendered = false;
                for (condition, body) in branches {
                    if condition_matches(condition, scope, None)? {
                        output.push_str(&render_scalar_nodes(body, scope)?);
                        rendered = true;
                        break;
                    }
                }
                if !rendered {
                    output.push_str(&render_scalar_nodes(else_body, scope)?);
                }
            }
        }
    }
    Ok(output)
}

fn resolve_path_expr(path: &PathExpr, scope: &Scope<'_>) -> Result<Value, TemplateError> {
    let mut value = resolve_path(scope, &path.base);
    for helper in &path.helpers {
        value = apply_helper(value, helper, scope)?;
    }
    Ok(value)
}

fn apply_helper(
    value: Value,
    helper: &HelperCall,
    scope: &Scope<'_>,
) -> Result<Value, TemplateError> {
    match helper {
        HelperCall::Length => Ok(json!(value_length(&value))),
        HelperCall::Join(separator) => Ok(Value::String(array_values(&value).map_or_else(
            || render_value(&value),
            |values| {
                values
                    .iter()
                    .map(render_value)
                    .collect::<Vec<_>>()
                    .join(separator)
            },
        ))),
        HelperCall::First => Ok(array_values(&value)
            .and_then(|values| values.first().cloned())
            .unwrap_or(Value::Null)),
        HelperCall::Last => Ok(array_values(&value)
            .and_then(|values| values.last().cloned())
            .unwrap_or(Value::Null)),
        HelperCall::Map(path) => Ok(Value::Array(
            array_values(&value)
                .unwrap_or_default()
                .into_iter()
                .map(|item| {
                    let item_scope = scope_with_item(scope, item);
                    resolve_path(&item_scope, path)
                })
                .collect(),
        )),
        HelperCall::Filter(condition) => Ok(Value::Array(
            array_values(&value)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|item| {
                    let matched = condition_matches(condition, scope, Some(&item)).unwrap_or(false);
                    matched.then_some(item)
                })
                .collect(),
        )),
        HelperCall::Count(condition) => {
            let values = array_values(&value).unwrap_or_default();
            let count = if let Some(condition) = condition {
                values
                    .iter()
                    .filter(|item| condition_matches(condition, scope, Some(item)).unwrap_or(false))
                    .count()
            } else {
                values.len()
            };
            Ok(json!(count))
        }
    }
}

fn condition_matches(
    condition: &str,
    scope: &Scope<'_>,
    item: Option<&Value>,
) -> Result<bool, TemplateError> {
    let Some(expression) = parse_expression(condition)
        .map_err(|error| TemplateError::single(format!("invalid condition expression: {error}")))?
    else {
        return Ok(false);
    };
    let context = expression_context(scope, item);
    Ok(expression_matches(&context, &expression))
}

fn expression_context(scope: &Scope<'_>, item: Option<&Value>) -> ExpressionContext {
    let mut context = ExpressionContext::default();
    for root in [
        "rule",
        "event",
        "query",
        "server",
        "job",
        "schedule",
        "alert",
        "policy",
        "policy_rule",
        "telemetry",
    ] {
        if let Some(value) = scope.root.get(root).cloned() {
            context = context.with_json_root(root, value);
        }
    }
    for (key, value) in &scope.locals {
        context
            .objects
            .insert(key.to_ascii_lowercase(), value.clone());
        if key == "vps" || looks_like_vps(value) {
            context.vps = vps_metadata_from_value(value);
        }
    }
    if let Some(item) = item {
        context.objects.insert("item".to_string(), item.clone());
        if looks_like_vps(item) {
            context.vps = vps_metadata_from_value(item);
            context.objects.insert("vps".to_string(), item.clone());
        }
    }
    if let Some(event) = scope.root.get("event") {
        if let Some(kind) = event.get("kind").and_then(Value::as_str) {
            context.event_predicates.insert(kind.to_ascii_lowercase());
        }
        for key in ["predicates", "event_predicates"] {
            if let Some(values) = event.get(key).and_then(Value::as_array) {
                for value in values.iter().filter_map(Value::as_str) {
                    context.event_predicates.insert(value.to_ascii_lowercase());
                }
            }
        }
    }
    context
}

fn scope_with_item<'a>(scope: &Scope<'a>, item: Value) -> Scope<'a> {
    let mut child = scope.clone();
    child.locals.insert("item".to_string(), item.clone());
    if looks_like_vps(&item) {
        child.locals.insert("vps".to_string(), item);
    }
    child
}

fn resolve_path(scope: &Scope<'_>, path: &str) -> Value {
    let segments = path
        .split('.')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return Value::Null;
    }
    if segments[0] == "vps" && !scope.locals.contains_key("vps") {
        let Some(values) = scope.root.get("matched_vps").and_then(Value::as_array) else {
            return Value::Null;
        };
        if segments.len() == 1 {
            return Value::Array(values.clone());
        }
        return Value::Array(
            values
                .iter()
                .map(|value| value_path(value, &segments[1..]).unwrap_or(Value::Null))
                .collect(),
        );
    }
    let Some(current) = scope
        .locals
        .get(segments[0])
        .or_else(|| scope.root.get(segments[0]))
    else {
        return Value::Null;
    };
    if segments.len() == 1 {
        return current.clone();
    }
    value_path(current, &segments[1..]).unwrap_or(Value::Null)
}

fn value_path(value: &Value, segments: &[&str]) -> Option<Value> {
    let mut current = value;
    for segment in segments {
        if let Value::Array(values) = current {
            return Some(Value::Array(
                values
                    .iter()
                    .map(|value| value_path(value, segments).unwrap_or(Value::Null))
                    .collect(),
            ));
        }
        let key = if *segment == "name" && current.get("display_name").is_some() {
            "display_name"
        } else {
            segment
        };
        current = current.get(key)?;
    }
    Some(current.clone())
}

fn array_values(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::Array(values) => Some(values.clone()),
        Value::Null => Some(Vec::new()),
        _ => None,
    }
}

fn value_length(value: &Value) -> usize {
    match value {
        Value::Array(values) => values.len(),
        Value::Object(values) => values.len(),
        Value::String(value) => value.chars().count(),
        Value::Bool(_) | Value::Number(_) | Value::Null => 0,
    }
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Array(values) => values
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(object) => render_object(object),
    }
}

fn render_object(object: &Map<String, Value>) -> String {
    let id = object.get("id").and_then(Value::as_str);
    let name = object
        .get("display_name")
        .or_else(|| object.get("name"))
        .and_then(Value::as_str);
    match (name, id) {
        (Some(name), Some(id)) => format!("{name} ({id})"),
        (Some(name), None) => name.to_string(),
        (None, Some(id)) => id.to_string(),
        (None, None) => Value::Object(object.clone()).to_string(),
    }
}

fn looks_like_vps(value: &Value) -> bool {
    value.get("id").and_then(Value::as_str).is_some()
        && (value.get("display_name").is_some() || value.get("status").is_some())
}

fn vps_metadata_from_value(value: &Value) -> Option<VpsMetadata> {
    Some(VpsMetadata {
        id: value.get("id")?.as_str()?.to_string(),
        display_name: value
            .get("display_name")
            .or_else(|| value.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tags: value
            .get("tags")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        registration_ip: string_field(value, "registration_ip"),
        last_ip: string_field(value, "last_ip"),
        last_seen_at: string_field(value, "last_seen_at"),
        internal_build_number: value.get("internal_build_number").and_then(Value::as_u64),
        stale_since: string_field(value, "stale_since"),
        stale_reason: string_field(value, "stale_reason"),
        extra: Some(value.clone()),
    })
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0] as char;
        let last = trimmed.as_bytes()[trimmed.len() - 1] as char;
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn end_tag_label(tag: &EndTag) -> &'static str {
    match tag {
        EndTag::EndFor => "endfor",
        EndTag::ElseIf(_) => "elseif",
        EndTag::Else => "else",
        EndTag::EndIf => "endif",
    }
}

pub fn template_referenced_paths(template: &str) -> Result<BTreeSet<String>, TemplateError> {
    let nodes = parse_template(template)?;
    let mut paths = BTreeSet::new();
    collect_paths(&nodes, &mut paths);
    Ok(paths)
}

fn collect_paths(nodes: &[Node], paths: &mut BTreeSet<String>) {
    for node in nodes {
        match node {
            Node::Text(_) => {}
            Node::Placeholder(path) => {
                paths.insert(path.base.clone());
                for helper in &path.helpers {
                    if let HelperCall::Map(path) = helper {
                        paths.insert(path.clone());
                    }
                }
            }
            Node::For { path, body, .. } => {
                paths.insert(path.base.clone());
                collect_paths(body, paths);
            }
            Node::If {
                branches,
                else_body,
            } => {
                for (_condition, body) in branches {
                    collect_paths(body, paths);
                }
                collect_paths(else_body, paths);
            }
        }
    }
}

#[cfg(test)]
#[path = "tests_template.rs"]
mod tests;
