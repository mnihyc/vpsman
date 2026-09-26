use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde_json::{json, Map, Value};

use crate::{
    expression_matches, parse_expression, Expression, ExpressionContext, Predicate, VpsMetadata,
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
    Split(String),
    Substr(i64, Option<i64>),
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
    literal_object_locals: BTreeSet<String>,
}

/// Optional rendering policy. Limits apply to the final rendered substitution,
/// after its helpers; object-path prefixes opt out of display-name shorthand.
#[derive(Clone, Copy, Debug)]
pub struct TemplateRenderOptions<'a> {
    pub max_message_bytes: usize,
    pub max_substitution_bytes: Option<usize>,
    pub literal_object_paths: &'a [&'a str],
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EndTag {
    EndFor,
    ElseIf(String),
    Else,
    EndIf,
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
        literal_object_locals: BTreeSet::new(),
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
    render_template_with_options(
        template,
        context,
        TemplateRenderOptions {
            max_message_bytes,
            max_substitution_bytes: None,
            literal_object_paths: &[],
        },
    )
}

pub fn render_template_with_options(
    template: &str,
    context: &Value,
    options: TemplateRenderOptions<'_>,
) -> Result<String, TemplateError> {
    let nodes = parse_template(template)?;
    let scope = Scope {
        root: context,
        locals: BTreeMap::new(),
        literal_object_locals: BTreeSet::new(),
    };
    let rendered = render_nodes(&nodes, &scope, options)?;
    if rendered.len() > options.max_message_bytes {
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
    let mut cursor = 0_usize;
    let (nodes, end_tag) = parse_nodes(input, &mut cursor, false)?;
    if let Some(end_tag) = end_tag {
        return Err(TemplateError::single(format!(
            "unexpected closing block tag {}",
            end_tag_label(&end_tag)
        )));
    }
    Ok(nodes)
}

fn parse_nodes(
    input: &str,
    cursor: &mut usize,
    in_block: bool,
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
                nodes.push(Node::Placeholder(parse_path_expr(raw)?));
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
        if let Some((variable, path)) = parse_for_tag(tag)? {
            *cursor += tag_len;
            let (body, end_tag) = parse_nodes(input, cursor, true)?;
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
        if let Some(condition) = parse_if_tag(tag)? {
            *cursor += tag_len;
            let mut branches = Vec::new();
            let (body, mut end_tag) = parse_nodes(input, cursor, true)?;
            branches.push((condition, body));
            let mut else_body = Vec::new();
            loop {
                match end_tag {
                    Some(EndTag::ElseIf(condition)) => {
                        validate_condition(&condition)?;
                        let (body, next) = parse_nodes(input, cursor, true)?;
                        branches.push((condition, body));
                        end_tag = next;
                    }
                    Some(EndTag::Else) => {
                        let (body, next) = parse_nodes(input, cursor, true)?;
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

fn parse_for_tag(tag: &str) -> Result<Option<(String, PathExpr)>, TemplateError> {
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
    Ok(Some((variable.to_string(), parse_path_expr(path)?)))
}

fn parse_if_tag(tag: &str) -> Result<Option<String>, TemplateError> {
    let Some(condition) = tag.strip_prefix("if ") else {
        return Ok(None);
    };
    let condition = condition.trim();
    if condition.is_empty() {
        return Err(TemplateError::single("if block is missing a condition"));
    }
    validate_condition(condition)?;
    Ok(Some(condition.to_string()))
}

fn validate_condition(condition: &str) -> Result<(), TemplateError> {
    parse_expression(condition)
        .map_err(|error| TemplateError::single(format!("invalid condition expression: {error}")))?;
    Ok(())
}

fn parse_path_expr(raw: &str) -> Result<PathExpr, TemplateError> {
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
        } else if tail.starts_with(".split(") {
            let (argument, next) = helper_argument(raw, cursor + ".split".len())?;
            helpers.push(HelperCall::Split(parse_string_helper_argument(argument)?));
            cursor = next;
        } else if tail.starts_with(".substr(") {
            let (argument, next) = helper_argument(raw, cursor + ".substr".len())?;
            let arguments = argument.split(',').map(str::trim).collect::<Vec<_>>();
            if !(1..=2).contains(&arguments.len()) {
                return Err(TemplateError::single(
                    "substr expects start and optional length",
                ));
            }
            let parse_index = |value: &str| {
                value
                    .parse::<i64>()
                    .map_err(|_| TemplateError::single("substr indices must be integers"))
            };
            let start = parse_index(arguments[0])?;
            let length = arguments
                .get(1)
                .map(|value| parse_index(value))
                .transpose()?;
            helpers.push(HelperCall::Substr(start, length));
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
            validate_condition(argument)?;
            helpers.push(HelperCall::Filter(argument.to_string()));
            cursor = next;
        } else if tail.starts_with(".count(") {
            let (argument, next) = helper_argument(raw, cursor + ".count".len())?;
            let argument = argument.trim();
            if argument.is_empty() {
                helpers.push(HelperCall::Count(None));
            } else {
                validate_condition(argument)?;
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

fn first_helper_start(raw: &str) -> Option<usize> {
    [
        ".length", ".join(", ".split(", ".substr(", ".first", ".last", ".map(", ".filter(",
        ".where(", ".count(", ".count",
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

fn render_nodes(
    nodes: &[Node],
    scope: &Scope<'_>,
    options: TemplateRenderOptions<'_>,
) -> Result<String, TemplateError> {
    let mut output = String::new();
    for node in nodes {
        match node {
            Node::Text(text) => output.push_str(text),
            Node::Placeholder(path) => {
                let (value, literal_objects) = resolve_path_expr_with_literal_paths(
                    path,
                    scope,
                    options.literal_object_paths,
                )?;
                let rendered = render_value_with_object_mode(&value, literal_objects);
                match options.max_substitution_bytes {
                    Some(limit) => output.push_str(&truncate_substitution(&rendered, limit)?),
                    None => output.push_str(&rendered),
                }
            }
            Node::For {
                variable,
                path,
                body,
            } => {
                let (iterable, literal_objects) = resolve_path_expr_with_literal_paths(
                    path,
                    scope,
                    options.literal_object_paths,
                )?;
                if let Value::Array(values) = iterable {
                    for value in values {
                        let mut child = scope.clone();
                        child.locals.insert(variable.clone(), value);
                        if literal_objects {
                            child.literal_object_locals.insert(variable.clone());
                        } else {
                            child.literal_object_locals.remove(variable);
                        }
                        output.push_str(&render_nodes(body, &child, options)?);
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
                        output.push_str(&render_nodes(body, scope, options)?);
                        rendered = true;
                        break;
                    }
                }
                if !rendered {
                    output.push_str(&render_nodes(else_body, scope, options)?);
                }
            }
        }
    }
    Ok(output)
}

fn literal_object_path(path: &str, scope: &Scope<'_>, literal_object_paths: &[&str]) -> bool {
    let segments = path_segments(path).collect::<Vec<_>>();
    if let Some(root) = segments.first() {
        if scope.locals.contains_key(*root) {
            return scope.literal_object_locals.contains(*root);
        }
    }
    literal_object_paths.iter().any(|prefix| {
        let prefix = path_segments(prefix).collect::<Vec<_>>();
        !prefix.is_empty() && segments.starts_with(&prefix)
    })
}

fn truncate_substitution(value: &str, limit: usize) -> Result<String, TemplateError> {
    if value.len() <= limit {
        return Ok(value.to_string());
    }
    let marker = |remaining| format!("...[{remaining} bytes remaining]");
    if marker(value.len()).len() > limit {
        return Err(TemplateError::single(
            "substitution limit cannot fit remaining-byte marker",
        ));
    }
    // On UTF-8 boundaries, prefix length plus the decimal marker length is
    // monotone while bytes remain. Select the longest prefix that fits.
    let mut end = 0;
    for (boundary, _) in value.char_indices() {
        if boundary + marker(value.len() - boundary).len() > limit {
            break;
        }
        end = boundary;
    }
    Ok(format!("{}{}", &value[..end], marker(value.len() - end)))
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
    resolve_path_expr_with_literal_paths(path, scope, &[]).map(|(value, _)| value)
}

fn resolve_path_expr_with_literal_paths(
    path: &PathExpr,
    scope: &Scope<'_>,
    literal_object_paths: &[&str],
) -> Result<(Value, bool), TemplateError> {
    let mut literal_objects = literal_object_path(&path.base, scope, literal_object_paths);
    let mut value = resolve_path_with_object_mode(scope, &path.base, literal_objects);
    for helper in &path.helpers {
        if let HelperCall::Map(argument) = helper {
            // Mapping changes a value's origin. Resolve each argument under its
            // own path policy, including local aliases, instead of inheriting
            // the outer collection's display-name/object policy.
            let mut mapped_literal_objects = false;
            let mut mapped = Vec::new();
            for item in array_values(&value).unwrap_or_default() {
                let item_scope = scope_with_item(scope, item, literal_objects);
                let argument_literal =
                    literal_object_path(argument, &item_scope, literal_object_paths);
                mapped_literal_objects |= argument_literal;
                mapped.push(resolve_path_with_object_mode(
                    &item_scope,
                    argument,
                    argument_literal,
                ));
            }
            value = Value::Array(mapped);
            literal_objects = mapped_literal_objects;
            continue;
        }
        value = apply_helper(value, helper, scope, literal_objects)?;
    }
    Ok((value, literal_objects))
}

fn apply_helper(
    value: Value,
    helper: &HelperCall,
    scope: &Scope<'_>,
    literal_objects: bool,
) -> Result<Value, TemplateError> {
    match helper {
        HelperCall::Length => Ok(json!(value_length(&value))),
        HelperCall::Join(separator) => Ok(Value::String(array_values(&value).map_or_else(
            || render_value_with_object_mode(&value, literal_objects),
            |values| {
                values
                    .iter()
                    .map(|value| render_value_with_object_mode(value, literal_objects))
                    .collect::<Vec<_>>()
                    .join(separator)
            },
        ))),
        HelperCall::Split(separator) => {
            let value = value
                .as_str()
                .ok_or_else(|| TemplateError::single("split helper requires a string"))?;
            let values = if separator.is_empty() {
                value
                    .chars()
                    .map(|character| Value::String(character.to_string()))
                    .collect()
            } else {
                value
                    .split(separator)
                    .map(|part| Value::String(part.to_string()))
                    .collect()
            };
            Ok(Value::Array(values))
        }
        HelperCall::Substr(start, length) => {
            let value = value
                .as_str()
                .ok_or_else(|| TemplateError::single("substr helper requires a string"))?;
            let count = value.chars().count();
            let start = if *start < 0 {
                count.saturating_sub(usize::try_from(start.unsigned_abs()).unwrap_or(usize::MAX))
            } else {
                usize::try_from(*start).unwrap_or(usize::MAX).min(count)
            };
            let length = length
                .map(|length| usize::try_from(length.max(0)).unwrap_or(usize::MAX))
                .unwrap_or(usize::MAX);
            Ok(Value::String(
                value.chars().skip(start).take(length).collect(),
            ))
        }
        HelperCall::First => Ok(array_values(&value)
            .and_then(|values| values.first().cloned())
            .unwrap_or(Value::Null)),
        HelperCall::Last => Ok(array_values(&value)
            .and_then(|values| values.last().cloned())
            .unwrap_or(Value::Null)),
        HelperCall::Map(_) => unreachable!("map arguments are resolved with their path policy"),
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

fn scope_with_item<'a>(scope: &Scope<'a>, item: Value, literal_objects: bool) -> Scope<'a> {
    let mut child = scope.clone();
    child.locals.insert("item".to_string(), item.clone());
    if literal_objects {
        child.literal_object_locals.insert("item".to_string());
    } else {
        child.literal_object_locals.remove("item");
    }
    if looks_like_vps(&item) {
        child.locals.insert("vps".to_string(), item);
        if literal_objects {
            child.literal_object_locals.insert("vps".to_string());
        } else {
            child.literal_object_locals.remove("vps");
        }
    }
    child
}

fn path_segments(path: &str) -> impl Iterator<Item = &str> {
    path.split('.')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
}

fn resolve_path_with_object_mode(scope: &Scope<'_>, path: &str, literal_objects: bool) -> Value {
    let segments = path_segments(path).collect::<Vec<_>>();
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
                .map(|value| {
                    value_path(value, &segments[1..], literal_objects).unwrap_or(Value::Null)
                })
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
    value_path(current, &segments[1..], literal_objects).unwrap_or(Value::Null)
}

fn value_path(value: &Value, segments: &[&str], literal_objects: bool) -> Option<Value> {
    let mut current = value;
    for segment in segments {
        if let Value::Array(values) = current {
            return Some(Value::Array(
                values
                    .iter()
                    .map(|value| {
                        value_path(value, segments, literal_objects).unwrap_or(Value::Null)
                    })
                    .collect(),
            ));
        }
        let key = if !literal_objects && *segment == "name" && current.get("display_name").is_some()
        {
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

fn render_value_with_object_mode(value: &Value, literal_objects: bool) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Array(values) => values
            .iter()
            .map(|value| render_value_with_object_mode(value, literal_objects))
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(object) if literal_objects => Value::Object(object.clone()).to_string(),
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

fn parse_string_helper_argument(argument: &str) -> Result<String, TemplateError> {
    let argument = argument.trim();
    if argument.is_empty() {
        return Err(TemplateError::single("split helper requires a separator"));
    }
    let encoded = if argument.starts_with('\'') {
        if !argument.ends_with('\'') || argument.len() < 2 {
            return Err(TemplateError::single("invalid quoted helper argument"));
        }
        let mut encoded = String::from("\"");
        let mut escaped = false;
        for character in argument[1..argument.len() - 1].chars() {
            if escaped {
                if character != '\'' {
                    encoded.push('\\');
                }
                encoded.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                encoded.push_str("\\\"");
            } else {
                encoded.push(character);
            }
        }
        if escaped {
            return Err(TemplateError::single("invalid quoted helper argument"));
        }
        encoded.push('"');
        encoded
    } else if argument.starts_with('"') {
        argument.to_string()
    } else {
        return Ok(argument.to_string());
    };
    serde_json::from_str(&encoded)
        .map_err(|error| TemplateError::single(format!("invalid quoted helper argument: {error}")))
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
    collect_paths(&nodes, &mut paths)?;
    Ok(paths)
}

fn collect_paths(nodes: &[Node], paths: &mut BTreeSet<String>) -> Result<(), TemplateError> {
    for node in nodes {
        match node {
            Node::Text(_) => {}
            Node::Placeholder(path) => {
                collect_path_expression(path, paths)?;
            }
            Node::For { path, body, .. } => {
                collect_path_expression(path, paths)?;
                collect_paths(body, paths)?;
            }
            Node::If {
                branches,
                else_body,
            } => {
                for (condition, body) in branches {
                    collect_condition_paths(condition, paths)?;
                    collect_paths(body, paths)?;
                }
                collect_paths(else_body, paths)?;
            }
        }
    }
    Ok(())
}

fn collect_path_expression(
    path: &PathExpr,
    paths: &mut BTreeSet<String>,
) -> Result<(), TemplateError> {
    paths.insert(path_segments(&path.base).collect::<Vec<_>>().join("."));
    for helper in &path.helpers {
        match helper {
            HelperCall::Map(path) => {
                paths.insert(path_segments(path).collect::<Vec<_>>().join("."));
            }
            HelperCall::Filter(condition) | HelperCall::Count(Some(condition)) => {
                collect_condition_paths(condition, paths)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn collect_condition_paths(
    condition: &str,
    paths: &mut BTreeSet<String>,
) -> Result<(), TemplateError> {
    fn visit(expression: &Expression, paths: &mut BTreeSet<String>) {
        match expression {
            Expression::Predicate(predicate) => match predicate {
                Predicate::Comparison { field: path, .. }
                | Predicate::Membership { field: path, .. } => {
                    paths.insert(path.clone());
                }
                Predicate::Bare(_) | Predicate::Event(_) | Predicate::Untagged => {}
            },
            Expression::Not(inner) => visit(inner, paths),
            Expression::And(left, right) | Expression::Or(left, right) => {
                visit(left, paths);
                visit(right, paths);
            }
        }
    }
    let expression =
        parse_expression(condition).map_err(|error| TemplateError::single(error.to_string()))?;
    if let Some(expression) = expression {
        visit(&expression, paths);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests_template.rs"]
mod tests;
