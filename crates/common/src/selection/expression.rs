use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;

use crate::{parse_vps_rule_value, SUPPORTED_VPS_RULE_KEYS};

#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    Predicate(Predicate),
    Not(Box<Expression>),
    And(Box<Expression>, Box<Expression>),
    Or(Box<Expression>, Box<Expression>),
}

/// Three-valued result used when an expression is evaluated against a partial
/// evidence document. The ordinary selector/webhook matcher intentionally
/// remains boolean; policy evaluation uses this type so absent facts cannot be
/// mistaken for a conclusive false (and therefore for recovery).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpressionTruth {
    True,
    False,
    Unknown,
}

impl ExpressionTruth {
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }

    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Predicate {
    Bare(String),
    Comparison {
        field: String,
        operator: ComparisonOperator,
        value: ScalarValue,
    },
    Membership {
        field: String,
        negated: bool,
        values: Vec<ListValue>,
    },
    Event(String),
    Untagged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparisonOperator {
    Eq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScalarValue {
    Literal(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ListValue {
    Literal(String),
    Regex(String),
}

#[derive(Clone, Debug, Default)]
pub struct ExpressionContext {
    pub vps: Option<VpsMetadata>,
    pub vps_rules: Option<VpsRuleContext>,
    pub rule: Option<Value>,
    pub event: Option<Value>,
    pub query: Option<Value>,
    pub server: Option<Value>,
    pub job: Option<Value>,
    pub schedule: Option<Value>,
    pub alert: Option<Value>,
    pub policy_rule: Option<Value>,
    pub telemetry: Option<Value>,
    pub objects: BTreeMap<String, Value>,
    pub event_predicates: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct VpsMetadata {
    pub id: String,
    pub display_name: String,
    pub status: String,
    pub tags: Vec<String>,
    pub registration_ip: Option<String>,
    pub last_ip: Option<String>,
    pub last_seen_at: Option<String>,
    pub internal_build_number: Option<u64>,
    pub stale_since: Option<String>,
    pub stale_reason: Option<String>,
    pub extra: Option<Value>,
}

pub const VPS_RULE_FIELD: &str = "vps.rules";

#[derive(Clone, Debug, PartialEq)]
pub struct VpsRuleValue {
    pub value_raw: String,
    pub value_json: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VpsRuleContext {
    pub values: BTreeMap<String, VpsRuleValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FieldValue {
    String(String),
    Number(i128),
    Decimal(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    And,
    Colon,
    Comma,
    Eq,
    Gt,
    Gte,
    In,
    LeftBracket,
    LeftParen,
    Lt,
    Lte,
    Not,
    NotEq,
    Or,
    Regex(String),
    RightBracket,
    RightParen,
    String(String),
    Word(String),
}

pub fn parse_expression(input: &str) -> Result<Option<Expression>, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Ok(None);
    }
    let mut parser = Parser {
        position: 0,
        tokens,
    };
    let expression = parser.parse_or()?;
    if parser.peek().is_some() {
        return Err("unexpected token after expression".to_string());
    }
    validate_expression(&expression)?;
    Ok(Some(expression))
}

pub fn expression_matches(context: &ExpressionContext, expression: &Expression) -> bool {
    match expression {
        Expression::Predicate(predicate) => predicate_matches(context, predicate),
        Expression::Not(inner) => !expression_matches(context, inner),
        Expression::And(left, right) => {
            expression_matches(context, left) && expression_matches(context, right)
        }
        Expression::Or(left, right) => {
            expression_matches(context, left) || expression_matches(context, right)
        }
    }
}

/// Evaluates the shared expression AST using strong Kleene logic.
///
/// A comparison or membership predicate whose field is absent, null, or a
/// non-scalar object is `Unknown`. An explicitly present empty array is known
/// and follows the normal membership semantics. This is the policy-evidence
/// boundary: `Unknown` pauses confirmation gates and may never resolve an
/// active condition through the inverse of its trigger expression.
pub fn expression_truth(context: &ExpressionContext, expression: &Expression) -> ExpressionTruth {
    match expression {
        Expression::Predicate(predicate) => predicate_truth(context, predicate),
        Expression::Not(inner) => expression_truth(context, inner).not(),
        Expression::And(left, right) => {
            expression_truth(context, left).and(expression_truth(context, right))
        }
        Expression::Or(left, right) => {
            expression_truth(context, left).or(expression_truth(context, right))
        }
    }
}

pub fn parse_and_match_expression(
    input: &str,
    context: &ExpressionContext,
) -> Result<bool, String> {
    Ok(parse_expression(input)?
        .as_ref()
        .is_none_or(|expression| expression_matches(context, expression)))
}

pub fn id_selector_expression(client_id: &str) -> String {
    format!("id:{}", client_id.trim())
}

pub fn expression_referenced_roots(expression: &Expression) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    collect_expression_roots(expression, &mut roots);
    roots
}

pub fn expression_referenced_events(expression: &Expression) -> BTreeSet<String> {
    let mut events = BTreeSet::new();
    collect_expression_events(expression, &mut events);
    events
}

pub fn expression_references_vps_rules(expression: &Expression) -> bool {
    match expression {
        Expression::Predicate(Predicate::Comparison { field, .. })
        | Expression::Predicate(Predicate::Membership { field, .. }) => is_vps_rule_field(field),
        Expression::Predicate(Predicate::Bare(raw)) => is_vps_rule_field(raw),
        Expression::Predicate(_) => false,
        Expression::Not(inner) => expression_references_vps_rules(inner),
        Expression::And(left, right) | Expression::Or(left, right) => {
            expression_references_vps_rules(left) || expression_references_vps_rules(right)
        }
    }
}

impl ExpressionContext {
    pub fn for_vps(vps: VpsMetadata) -> Self {
        Self {
            vps: Some(vps),
            ..Self::default()
        }
    }

    pub fn with_event_predicate(mut self, predicate: impl Into<String>) -> Self {
        self.event_predicates
            .insert(predicate.into().to_ascii_lowercase());
        self
    }

    pub fn with_vps_rules(mut self, vps_rules: VpsRuleContext) -> Self {
        self.vps_rules = Some(vps_rules);
        self
    }

    pub fn with_json_root(mut self, root: impl Into<String>, value: Value) -> Self {
        let root = root.into().to_ascii_lowercase();
        match root.as_str() {
            "rule" => self.rule = Some(value),
            "event" => self.event = Some(value),
            "query" => self.query = Some(value),
            "server" => self.server = Some(value),
            "job" => self.job = Some(value),
            "schedule" => self.schedule = Some(value),
            "alert" => self.alert = Some(value),
            "policy_rule" => self.policy_rule = Some(value),
            "telemetry" => self.telemetry = Some(value),
            _ => {
                self.objects.insert(root, value);
            }
        }
        self
    }
}

impl VpsRuleValue {
    pub fn new(value_raw: impl Into<String>, value_json: Value) -> Self {
        Self {
            value_raw: value_raw.into(),
            value_json,
        }
    }
}

impl VpsRuleContext {
    pub fn from_values(values: impl IntoIterator<Item = (String, VpsRuleValue)>) -> Self {
        let mut context = Self::default();
        for (key, value) in values {
            context
                .values
                .insert(key.trim().to_ascii_lowercase(), value);
        }
        context
    }

    pub fn insert(
        &mut self,
        key: impl Into<String>,
        value_raw: impl Into<String>,
        value_json: Value,
    ) -> Option<VpsRuleValue> {
        self.values.insert(
            key.into().trim().to_ascii_lowercase(),
            VpsRuleValue::new(value_raw, value_json),
        )
    }
}

impl VpsMetadata {
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        status: impl Into<String>,
        tags: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            status: status.into(),
            tags,
            ..Self::default()
        }
    }
}

fn tokenize(input: &str) -> Result<Vec<TokenKind>, String> {
    let mut tokens = Vec::new();
    let chars = input.char_indices().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        let (_, character) = chars[index];
        if character.is_whitespace() {
            index += 1;
            continue;
        }
        match character {
            '(' => {
                tokens.push(TokenKind::LeftParen);
                index += 1;
                continue;
            }
            ')' => {
                tokens.push(TokenKind::RightParen);
                index += 1;
                continue;
            }
            '[' => {
                tokens.push(TokenKind::LeftBracket);
                index += 1;
                continue;
            }
            ']' => {
                tokens.push(TokenKind::RightBracket);
                index += 1;
                continue;
            }
            ',' => {
                tokens.push(TokenKind::Comma);
                index += 1;
                continue;
            }
            ':' => {
                tokens.push(TokenKind::Colon);
                index += 1;
                continue;
            }
            '~' => {
                tokens.push(TokenKind::Not);
                index += 1;
                continue;
            }
            '!' => {
                if chars.get(index + 1).is_some_and(|(_, next)| *next == '=') {
                    tokens.push(TokenKind::NotEq);
                    index += 2;
                } else {
                    tokens.push(TokenKind::Not);
                    index += 1;
                }
                continue;
            }
            '=' => {
                tokens.push(TokenKind::Eq);
                index += 1;
                continue;
            }
            '<' => {
                if chars.get(index + 1).is_some_and(|(_, next)| *next == '=') {
                    tokens.push(TokenKind::Lte);
                    index += 2;
                } else {
                    tokens.push(TokenKind::Lt);
                    index += 1;
                }
                continue;
            }
            '>' => {
                if chars.get(index + 1).is_some_and(|(_, next)| *next == '=') {
                    tokens.push(TokenKind::Gte);
                    index += 2;
                } else {
                    tokens.push(TokenKind::Gt);
                    index += 1;
                }
                continue;
            }
            '&' => {
                if chars.get(index + 1).is_some_and(|(_, next)| *next == '&') {
                    tokens.push(TokenKind::And);
                    index += 2;
                    continue;
                }
                return Err("use && or || for boolean operators".to_string());
            }
            '|' => {
                if chars.get(index + 1).is_some_and(|(_, next)| *next == '|') {
                    tokens.push(TokenKind::Or);
                    index += 2;
                    continue;
                }
                return Err("use && or || for boolean operators".to_string());
            }
            '"' | '\'' => {
                let (value, next_index) = read_quoted(input, &chars, index, character)?;
                tokens.push(TokenKind::String(value));
                index = next_index;
                continue;
            }
            '/' => {
                let (value, next_index) = read_regex(input, &chars, index)?;
                tokens.push(TokenKind::Regex(value));
                index = next_index;
                continue;
            }
            _ => {}
        }

        let (word, next_index) = read_word(input, &chars, index)?;
        let raw = word.trim();
        if raw.is_empty() {
            index = next_index;
            continue;
        }
        tokens.push(match raw.to_ascii_lowercase().as_str() {
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            "in" => TokenKind::In,
            _ => TokenKind::Word(raw.to_string()),
        });
        index = next_index;
    }
    Ok(tokens)
}

fn read_word(
    input: &str,
    chars: &[(usize, char)],
    start_index: usize,
) -> Result<(String, usize), String> {
    let mut value = String::new();
    let mut quote = None;
    let mut quote_start = None;
    let mut escaped = false;
    let mut cursor = start_index;
    while cursor < chars.len() {
        let (byte_index, current) = chars[cursor];
        if let Some(active_quote) = quote {
            if escaped {
                value.push(current);
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if current == active_quote {
                quote = None;
                quote_start = None;
            } else {
                value.push(current);
            }
            cursor += 1;
            continue;
        }
        if current.is_whitespace()
            || matches!(
                current,
                '(' | ')' | '[' | ']' | ',' | '=' | '!' | '<' | '>' | '&' | '|' | '~'
            )
        {
            break;
        }
        if current == '"' || current == '\'' {
            quote = Some(current);
            quote_start = Some(byte_index);
            cursor += 1;
            continue;
        }
        value.push(current);
        cursor += 1;
    }
    if quote.is_some() {
        return Err(format!(
            "unterminated quoted value starting at byte {}",
            quote_start.unwrap_or(input.len())
        ));
    }
    Ok((value, cursor))
}

fn read_quoted(
    _input: &str,
    chars: &[(usize, char)],
    start_index: usize,
    quote: char,
) -> Result<(String, usize), String> {
    let mut value = String::new();
    let mut escaped = false;
    let mut cursor = start_index + 1;
    while cursor < chars.len() {
        let (_, current) = chars[cursor];
        if escaped {
            value.push(current);
            escaped = false;
        } else if current == '\\' {
            escaped = true;
        } else if current == quote {
            return Ok((value, cursor + 1));
        } else {
            value.push(current);
        }
        cursor += 1;
    }
    Err(format!(
        "unterminated quoted value starting at byte {}",
        chars[start_index].0
    ))
}

fn read_regex(
    _input: &str,
    chars: &[(usize, char)],
    start_index: usize,
) -> Result<(String, usize), String> {
    let mut pattern = String::new();
    let mut escaped = false;
    let mut cursor = start_index + 1;
    while cursor < chars.len() {
        let (_, current) = chars[cursor];
        if escaped {
            pattern.push('\\');
            pattern.push(current);
            escaped = false;
        } else if current == '\\' {
            escaped = true;
        } else if current == '/' {
            let next = cursor + 1;
            if chars
                .get(next)
                .is_some_and(|(_, flag)| flag.is_ascii_alphabetic())
            {
                return Err("regex flags are not supported".to_string());
            }
            return Ok((pattern, next));
        } else {
            pattern.push(current);
        }
        cursor += 1;
    }
    Err(format!(
        "unterminated regex value starting at byte {}",
        chars[start_index].0
    ))
}

struct Parser {
    tokens: Vec<TokenKind>,
    position: usize,
}

impl Parser {
    fn parse_or(&mut self) -> Result<Expression, String> {
        let mut expression = self.parse_and()?;
        while self.consume_or() {
            let right = self.parse_and()?;
            expression = Expression::Or(Box::new(expression), Box::new(right));
        }
        Ok(expression)
    }

    fn parse_and(&mut self) -> Result<Expression, String> {
        let mut expression = self.parse_not()?;
        loop {
            if self.consume_and() {
                let right = self.parse_not()?;
                expression = Expression::And(Box::new(expression), Box::new(right));
                continue;
            }
            if self.next_starts_primary() {
                let right = self.parse_not()?;
                expression = Expression::And(Box::new(expression), Box::new(right));
                continue;
            }
            break;
        }
        Ok(expression)
    }

    fn parse_not(&mut self) -> Result<Expression, String> {
        if self.consume_not() {
            return Ok(Expression::Not(Box::new(self.parse_not()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expression, String> {
        match self.advance() {
            Some(TokenKind::Word(raw)) => self.parse_predicate(raw).map(Expression::Predicate),
            Some(TokenKind::LeftParen) => {
                let expression = self.parse_or()?;
                if !matches!(self.advance(), Some(TokenKind::RightParen)) {
                    return Err("missing closing parenthesis".to_string());
                }
                Ok(expression)
            }
            Some(TokenKind::And | TokenKind::Or | TokenKind::In) => {
                Err("operator is missing a left operand".to_string())
            }
            Some(TokenKind::Not) => unreachable!("parse_not consumes unary not"),
            Some(TokenKind::RightParen) => Err("unexpected closing parenthesis".to_string()),
            Some(_) => Err("predicate must start with a field or shorthand term".to_string()),
            None => Err("expression is incomplete".to_string()),
        }
    }

    fn parse_predicate(&mut self, raw: String) -> Result<Predicate, String> {
        if let Some(operator) = self.consume_comparison_operator() {
            let value = self.parse_scalar_value()?;
            let field = canonical_field(&raw);
            return Ok(Predicate::Comparison {
                field,
                operator,
                value,
            });
        }
        if self.consume_in() {
            let values = self.parse_list_values()?;
            let field = canonical_field(&raw);
            return Ok(Predicate::Membership {
                field,
                negated: false,
                values,
            });
        }
        if self.consume_not_in() {
            let values = self.parse_list_values()?;
            let field = canonical_field(&raw);
            return Ok(Predicate::Membership {
                field,
                negated: true,
                values,
            });
        }
        if raw.eq_ignore_ascii_case("untagged") {
            return Ok(Predicate::Untagged);
        }
        if is_event_predicate(&raw) {
            return Ok(Predicate::Event(raw.to_ascii_lowercase()));
        }
        if let Some((namespace, value)) = raw.split_once(':') {
            if namespace.is_empty() {
                return Err("selector namespace is empty".to_string());
            }
            if value.is_empty() {
                return Err("selector value is empty".to_string());
            }
            return shorthand_predicate(namespace, value);
        }
        Ok(Predicate::Bare(raw))
    }

    fn parse_scalar_value(&mut self) -> Result<ScalarValue, String> {
        match self.advance() {
            Some(TokenKind::Word(value) | TokenKind::String(value)) => {
                Ok(ScalarValue::Literal(value))
            }
            Some(TokenKind::Regex(_)) => {
                Err("regex scalar values are only supported in lists".to_string())
            }
            Some(_) => Err("comparison is missing a scalar value".to_string()),
            None => Err("comparison is missing a scalar value".to_string()),
        }
    }

    fn parse_list_values(&mut self) -> Result<Vec<ListValue>, String> {
        if !matches!(self.advance(), Some(TokenKind::LeftBracket)) {
            return Err("membership comparison is missing [".to_string());
        }
        let mut values = Vec::new();
        loop {
            match self.advance() {
                Some(TokenKind::Word(value) | TokenKind::String(value)) => {
                    values.push(ListValue::Literal(value));
                }
                Some(TokenKind::Regex(pattern)) => {
                    Regex::new(&pattern)
                        .map_err(|error| format!("invalid regex list value: {error}"))?;
                    values.push(ListValue::Regex(pattern));
                }
                Some(TokenKind::RightBracket) if values.is_empty() => {
                    return Err("membership list must not be empty".to_string());
                }
                Some(_) => return Err("membership list contains an invalid value".to_string()),
                None => return Err("membership list is missing ]".to_string()),
            }
            match self.peek() {
                Some(TokenKind::Comma) => {
                    self.position += 1;
                }
                Some(TokenKind::RightBracket) => {
                    self.position += 1;
                    break;
                }
                Some(_) => return Err("membership list values must be comma-separated".to_string()),
                None => return Err("membership list is missing ]".to_string()),
            }
        }
        Ok(values)
    }

    fn consume_and(&mut self) -> bool {
        if matches!(self.peek(), Some(TokenKind::And)) {
            self.position += 1;
            return true;
        }
        false
    }

    fn consume_or(&mut self) -> bool {
        if matches!(self.peek(), Some(TokenKind::Or)) {
            self.position += 1;
            return true;
        }
        false
    }

    fn consume_not(&mut self) -> bool {
        if matches!(self.peek(), Some(TokenKind::Not)) {
            self.position += 1;
            return true;
        }
        false
    }

    fn consume_in(&mut self) -> bool {
        if matches!(self.peek(), Some(TokenKind::In)) {
            self.position += 1;
            return true;
        }
        false
    }

    fn consume_not_in(&mut self) -> bool {
        if !matches!(self.peek(), Some(TokenKind::Not)) {
            return false;
        }
        if !matches!(self.tokens.get(self.position + 1), Some(TokenKind::In)) {
            return false;
        }
        self.position += 2;
        true
    }

    fn consume_comparison_operator(&mut self) -> Option<ComparisonOperator> {
        let operator = match self.peek()? {
            TokenKind::Eq => ComparisonOperator::Eq,
            TokenKind::NotEq => ComparisonOperator::NotEq,
            TokenKind::Lt => ComparisonOperator::Lt,
            TokenKind::Lte => ComparisonOperator::Lte,
            TokenKind::Gt => ComparisonOperator::Gt,
            TokenKind::Gte => ComparisonOperator::Gte,
            _ => return None,
        };
        self.position += 1;
        Some(operator)
    }

    fn next_starts_primary(&self) -> bool {
        matches!(
            self.peek(),
            Some(TokenKind::Word(_) | TokenKind::LeftParen | TokenKind::Not)
        )
    }

    fn advance(&mut self) -> Option<TokenKind> {
        let token = self.tokens.get(self.position)?.clone();
        self.position += 1;
        Some(token)
    }

    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.position)
    }
}

fn shorthand_predicate(namespace: &str, value: &str) -> Result<Predicate, String> {
    let namespace_lower = namespace.to_ascii_lowercase();
    let field = match namespace_lower.as_str() {
        "id" => "vps.id".to_string(),
        "name" => "vps.display_name".to_string(),
        "status" => "vps.status".to_string(),
        "tag" | "tags" | "vps.tag" | "vps.tags" => "vps.tag".to_string(),
        "provider" => {
            return Ok(Predicate::Membership {
                field: "vps.tag".to_string(),
                negated: false,
                values: vec![ListValue::Literal(format!("provider:{value}"))],
            });
        }
        "country" => {
            return Ok(Predicate::Membership {
                field: "vps.tag".to_string(),
                negated: false,
                values: vec![ListValue::Literal(format!("country:{value}"))],
            });
        }
        "region" => {
            return Ok(Predicate::Membership {
                field: "vps.tag".to_string(),
                negated: false,
                values: vec![ListValue::Literal(format!("region:{value}"))],
            });
        }
        _ if namespace_lower.starts_with("vps.") => canonical_field(namespace),
        _ if is_event_field_alias(&namespace_lower) => canonical_field(namespace),
        _ => {
            return Ok(Predicate::Membership {
                field: "vps.tag".to_string(),
                negated: false,
                values: vec![ListValue::Literal(format!("{namespace}:{value}"))],
            });
        }
    };
    if matches!(field.as_str(), "vps.tag" | "vps.tags") {
        return Ok(Predicate::Membership {
            field: "vps.tag".to_string(),
            negated: false,
            values: vec![ListValue::Literal(value.to_string())],
        });
    }
    Ok(Predicate::Comparison {
        field,
        operator: ComparisonOperator::Eq,
        value: ScalarValue::Literal(value.to_string()),
    })
}

fn is_event_predicate(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.starts_with("interval.")
        || lower.starts_with("vps.status.")
        || lower.starts_with("vps.tag_event:")
        || lower.starts_with("vps.tag_event.added:")
        || lower.starts_with("vps.tag_event.removed:")
        || lower.starts_with("job.status:")
        || lower.starts_with("job.status.become_")
        || lower.starts_with("job.type:")
        || lower.starts_with("job.target.status:")
        || lower.starts_with("schedule.id:")
        || lower.starts_with("schedule.name:")
        || lower.starts_with("alert.severity:")
        || lower.starts_with("alert.category:")
        || matches!(
            lower.as_str(),
            "server.on_start"
                | "schedule.due"
                | "schedule.job_finished"
                | "schedule.failed"
                | "vps.tag_changed"
                | "job.created"
                | "alert.triggered"
                | "alert.resolved"
                | "telemetry.rollup"
                | "telemetry.network_rate"
                | "telemetry.tunnel"
        )
}

fn is_event_field_alias(namespace: &str) -> bool {
    namespace.starts_with("job.")
        || namespace.starts_with("schedule.")
        || namespace.starts_with("alert.")
        || namespace.starts_with("telemetry.")
}

fn canonical_field(field: &str) -> String {
    match field.to_ascii_lowercase().as_str() {
        "id" | "client_id" | "vps.id" | "vps.client_id" => "vps.id".to_string(),
        "name" | "display_name" | "vps.name" | "vps.display_name" => "vps.display_name".to_string(),
        "status" | "vps.status" => "vps.status".to_string(),
        "tag" | "tags" | "vps.tag" | "vps.tags" => "vps.tag".to_string(),
        "last_seen" | "last_seen_at" | "vps.last_seen" | "vps.last_seen_at" => {
            "vps.last_seen_at".to_string()
        }
        "region" | "vps.region" => "vps.region".to_string(),
        other => other.to_string(),
    }
}

pub fn validate_expression(expression: &Expression) -> Result<(), String> {
    match expression {
        Expression::Predicate(predicate) => validate_predicate(predicate),
        Expression::Not(inner) => validate_expression(inner),
        Expression::And(left, right) | Expression::Or(left, right) => {
            validate_expression(left)?;
            validate_expression(right)
        }
    }
}

fn validate_predicate(predicate: &Predicate) -> Result<(), String> {
    match predicate {
        Predicate::Comparison {
            field,
            operator,
            value: ScalarValue::Literal(expected),
        } => validate_vps_rule_comparison(field, *operator, expected),
        Predicate::Membership {
            field,
            negated,
            values,
        } => validate_vps_rule_membership(field, *negated, values),
        Predicate::Bare(raw) => validate_vps_rule_bare(raw),
        Predicate::Event(_) | Predicate::Untagged => Ok(()),
    }
}

fn validate_vps_rule_bare(raw: &str) -> Result<(), String> {
    let raw = raw.to_ascii_lowercase();
    if raw == VPS_RULE_FIELD || raw.starts_with("vps.rules.") {
        return Err(
            "choose a VPS rule; use vps.rules:<key>, for example vps.rules:traffic.reset_day"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_vps_rule_comparison(
    field: &str,
    operator: ComparisonOperator,
    expected: &str,
) -> Result<(), String> {
    let field = field.to_ascii_lowercase();
    if field == VPS_RULE_FIELD {
        return match operator {
            ComparisonOperator::Eq => validate_vps_rule_key_pattern(expected),
            ComparisonOperator::NotEq => {
                Err("vps.rules key absence uses !(vps.rules:<key>), not !=".to_string())
            }
            ComparisonOperator::Lt
            | ComparisonOperator::Lte
            | ComparisonOperator::Gt
            | ComparisonOperator::Gte => {
                Err("vps.rules key presence does not support ordered comparisons".to_string())
            }
        };
    }
    let Some(key) = field.strip_prefix("vps.rules:") else {
        if field.starts_with("vps.rules.") {
            return Err(
                "VPS rule values use vps.rules:<key>, for example vps.rules:traffic.reset_day"
                    .to_string(),
            );
        }
        return Ok(());
    };
    validate_vps_rule_key(key)?;
    if matches!(operator, ComparisonOperator::Eq | ComparisonOperator::NotEq)
        && !expected.contains('*')
        && !expected.contains('?')
    {
        parse_vps_rule_value(key, expected)
            .map_err(|error| format!("invalid value for VPS rule `{key}`: {error}"))?;
    }
    if !is_order_operator(operator) {
        return Ok(());
    }
    validate_vps_rule_order_rhs(key, expected)
}

fn validate_vps_rule_membership(
    field: &str,
    negated: bool,
    values: &[ListValue],
) -> Result<(), String> {
    let field = field.to_ascii_lowercase();
    if field == VPS_RULE_FIELD {
        if negated {
            return Err(
                "vps.rules key absence uses unary ! around a presence expression, not `not in`"
                    .to_string(),
            );
        }
        for value in values {
            if let ListValue::Literal(value) = value {
                validate_vps_rule_key_pattern(value)?;
            }
        }
        return Ok(());
    }
    if let Some(key) = field.strip_prefix("vps.rules:") {
        validate_vps_rule_key(key)?;
        for value in values {
            if let ListValue::Literal(value) = value {
                if !value.contains('*') && !value.contains('?') {
                    parse_vps_rule_value(key, value)
                        .map_err(|error| format!("invalid value for VPS rule `{key}`: {error}"))?;
                }
            }
        }
        return Ok(());
    }
    if field.starts_with("vps.rules.") {
        return Err(
            "VPS rule values use vps.rules:<key>, for example vps.rules:traffic.reset_day"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_vps_rule_key_pattern(pattern: &str) -> Result<(), String> {
    if pattern.contains('*') || pattern.contains('?') {
        return Ok(());
    }
    validate_vps_rule_key(&pattern.to_ascii_lowercase())
}

fn validate_vps_rule_key(key: &str) -> Result<(), String> {
    if SUPPORTED_VPS_RULE_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(format!("unsupported VPS rule key `{key}`"))
    }
}

fn validate_vps_rule_order_rhs(key: &str, expected: &str) -> Result<(), String> {
    let valid = match key {
        "traffic.reset_day" => expected_vps_rule_json_integer(key, expected, "day")
            .is_some_and(|value| (1..=31).contains(&value)),
        "traffic.quota.total" | "traffic.quota.rx" | "traffic.quota.tx" => {
            expected_vps_rule_json_integer(key, expected, "bytes").is_some_and(|value| value >= 0)
        }
        "network.port_speed" => {
            expected_vps_rule_json_integer(key, expected, "bps").is_some_and(|value| value > 0)
        }
        "billing.price" => parse_billing_price_order_value(expected).is_some(),
        _ => {
            return Err(format!(
                "VPS rule `{key}` does not support ordered comparisons"
            ));
        }
    };
    if valid {
        return Ok(());
    }
    let requirement = match key {
        "traffic.reset_day" => "a day from 1 to 31",
        "traffic.quota.total" | "traffic.quota.rx" | "traffic.quota.tx" => {
            "a positive byte size using B, KB, MB, GB, TB, KiB, MiB, GiB, or TiB"
        }
        "network.port_speed" => "a positive speed using bps, Kbps, Mbps, Gbps, or Tbps",
        "billing.price" => "a positive amount with currency and period, such as 50 USD/m",
        _ => unreachable!("unsupported ordered keys return above"),
    };
    Err(format!(
        "VPS rule `{key}` ordered comparison requires {requirement}"
    ))
}

fn is_order_operator(operator: ComparisonOperator) -> bool {
    matches!(
        operator,
        ComparisonOperator::Lt
            | ComparisonOperator::Lte
            | ComparisonOperator::Gt
            | ComparisonOperator::Gte
    )
}

fn is_vps_rule_field(field: &str) -> bool {
    let field = field.to_ascii_lowercase();
    field == VPS_RULE_FIELD || field.starts_with("vps.rules:") || field.starts_with("vps.rules.")
}

fn predicate_matches(context: &ExpressionContext, predicate: &Predicate) -> bool {
    match predicate {
        Predicate::Bare(raw) => bare_matches(context, raw),
        Predicate::Comparison {
            field,
            operator,
            value,
        } => comparison_matches(context, field, *operator, value),
        Predicate::Membership {
            field,
            negated,
            values,
        } => membership_matches(context, field, *negated, values),
        Predicate::Event(name) => context.event_predicates.contains(name),
        Predicate::Untagged => context.vps.as_ref().is_some_and(|vps| vps.tags.is_empty()),
    }
}

fn predicate_truth(context: &ExpressionContext, predicate: &Predicate) -> ExpressionTruth {
    match predicate {
        Predicate::Bare(raw) => {
            if context.vps.is_none() {
                ExpressionTruth::Unknown
            } else if bare_matches(context, raw) {
                ExpressionTruth::True
            } else {
                ExpressionTruth::False
            }
        }
        Predicate::Comparison {
            field,
            operator,
            value,
        } => {
            if field_values(context, field).is_none() {
                ExpressionTruth::Unknown
            } else if comparison_matches(context, field, *operator, value) {
                ExpressionTruth::True
            } else {
                ExpressionTruth::False
            }
        }
        Predicate::Membership {
            field,
            negated,
            values,
        } => {
            if field_values(context, field).is_none() {
                ExpressionTruth::Unknown
            } else if membership_matches(context, field, *negated, values) {
                ExpressionTruth::True
            } else {
                ExpressionTruth::False
            }
        }
        Predicate::Event(name) => {
            if context.event_predicates.contains(name) {
                ExpressionTruth::True
            } else {
                ExpressionTruth::False
            }
        }
        Predicate::Untagged => match context.vps.as_ref() {
            Some(vps) if vps.tags.is_empty() => ExpressionTruth::True,
            Some(_) => ExpressionTruth::False,
            None => ExpressionTruth::Unknown,
        },
    }
}

fn bare_matches(context: &ExpressionContext, raw: &str) -> bool {
    let Some(vps) = context.vps.as_ref() else {
        return false;
    };
    literal_matches(&vps.id, raw, true) || literal_matches(&vps.display_name, raw, true)
}

fn comparison_matches(
    context: &ExpressionContext,
    field: &str,
    operator: ComparisonOperator,
    value: &ScalarValue,
) -> bool {
    let ScalarValue::Literal(expected) = value;
    if matches!(operator, ComparisonOperator::Eq | ComparisonOperator::NotEq)
        && !expected.contains('*')
        && !expected.contains('?')
        && field.to_ascii_lowercase().starts_with("vps.rules:")
    {
        return vps_rule_exact_comparison_matches(
            context.vps_rules.as_ref(),
            field,
            operator,
            expected,
        );
    }
    if is_order_operator(operator) && field.to_ascii_lowercase().starts_with("vps.rules:") {
        return vps_rule_order_matches(context.vps_rules.as_ref(), field, operator, expected);
    }
    let Some(values) = field_values(context, field) else {
        return false;
    };
    values
        .iter()
        .any(|actual| compare_field_value(actual, operator, expected))
}

fn membership_matches(
    context: &ExpressionContext,
    field: &str,
    negated: bool,
    values: &[ListValue],
) -> bool {
    if field.to_ascii_lowercase().starts_with("vps.rules:") {
        return vps_rule_membership_matches(context.vps_rules.as_ref(), field, negated, values);
    }
    let Some(actual_values) = field_values(context, field) else {
        return false;
    };
    let matched = actual_values.iter().any(|actual| {
        let actual = match actual {
            FieldValue::String(value) => value.as_str(),
            FieldValue::Number(value) => return list_values_match(&value.to_string(), values),
            FieldValue::Decimal(value) => value.as_str(),
        };
        list_values_match(actual, values)
    });
    if negated {
        !matched
    } else {
        matched
    }
}

fn compare_field_value(actual: &FieldValue, operator: ComparisonOperator, expected: &str) -> bool {
    match operator {
        ComparisonOperator::Eq => field_value_literal_matches(actual, expected),
        ComparisonOperator::NotEq => !field_value_literal_matches(actual, expected),
        ComparisonOperator::Lt
        | ComparisonOperator::Lte
        | ComparisonOperator::Gt
        | ComparisonOperator::Gte => order_field_value(actual, operator, expected),
    }
}

fn field_value_literal_matches(actual: &FieldValue, expected: &str) -> bool {
    match actual {
        FieldValue::String(actual) => literal_matches(actual, expected, false),
        FieldValue::Number(actual) => expected
            .parse::<i128>()
            .is_ok_and(|expected| *actual == expected),
        FieldValue::Decimal(actual) => {
            let Some(actual) = actual.parse::<f64>().ok().filter(|value| value.is_finite()) else {
                return false;
            };
            expected
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .is_some_and(|expected| actual == expected)
        }
    }
}

fn order_field_value(actual: &FieldValue, operator: ComparisonOperator, expected: &str) -> bool {
    if let FieldValue::Number(actual) = actual {
        if let Ok(expected) = expected.parse::<i128>() {
            return compare_order(*actual, expected, operator);
        }
    }
    if let FieldValue::Decimal(actual) = actual {
        let values = actual
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .zip(
                expected
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite()),
            );
        if let Some((actual, expected)) = values {
            return match operator {
                ComparisonOperator::Lt => actual < expected,
                ComparisonOperator::Lte => actual <= expected,
                ComparisonOperator::Gt => actual > expected,
                ComparisonOperator::Gte => actual >= expected,
                ComparisonOperator::Eq | ComparisonOperator::NotEq => {
                    unreachable!("not an order operator")
                }
            };
        }
    }
    if let Some(actual) = field_value_timestamp(actual) {
        if let Some(expected) = parse_timestamp(expected) {
            return compare_order(actual, expected, operator);
        }
    }
    false
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BillingPriceOrderValue {
    amount_minor: i128,
    currency: String,
    period: String,
}

fn vps_rule_exact_comparison_matches(
    rules: Option<&VpsRuleContext>,
    field: &str,
    operator: ComparisonOperator,
    expected: &str,
) -> bool {
    let field = field.to_ascii_lowercase();
    let Some(key) = field.strip_prefix("vps.rules:") else {
        return false;
    };
    let Some(rule) = rules.and_then(|rules| rules.values.get(key)) else {
        return false;
    };
    let Some((actual, expected)) = normalized_rule_values(key, rule, expected) else {
        return false;
    };
    let matched = actual.eq_ignore_ascii_case(&expected);
    match operator {
        ComparisonOperator::Eq => matched,
        ComparisonOperator::NotEq => !matched,
        _ => false,
    }
}

fn vps_rule_membership_matches(
    rules: Option<&VpsRuleContext>,
    field: &str,
    negated: bool,
    values: &[ListValue],
) -> bool {
    let field = field.to_ascii_lowercase();
    let Some(key) = field.strip_prefix("vps.rules:") else {
        return false;
    };
    let Some(rule) = rules.and_then(|rules| rules.values.get(key)) else {
        return false;
    };
    let matched = values.iter().any(|expected| match expected {
        ListValue::Regex(pattern) => {
            Regex::new(pattern).is_ok_and(|regex| regex.is_match(&rule.value_raw))
        }
        ListValue::Literal(pattern) if pattern.contains('*') || pattern.contains('?') => {
            literal_matches(&rule.value_raw, pattern, false)
        }
        ListValue::Literal(expected) => normalized_rule_values(key, rule, expected)
            .is_some_and(|(actual, expected)| actual.eq_ignore_ascii_case(&expected)),
    });
    if negated {
        !matched
    } else {
        matched
    }
}

fn normalized_rule_values(
    key: &str,
    actual: &VpsRuleValue,
    expected: &str,
) -> Option<(String, String)> {
    let actual = parse_vps_rule_value(key, &actual.value_raw).ok()?.raw;
    let expected = parse_vps_rule_value(key, expected).ok()?.raw;
    Some((actual, expected))
}

fn vps_rule_order_matches(
    rules: Option<&VpsRuleContext>,
    field: &str,
    operator: ComparisonOperator,
    expected: &str,
) -> bool {
    let field = field.to_ascii_lowercase();
    let Some(key) = field.strip_prefix("vps.rules:") else {
        return false;
    };
    let Some(rule) = rules.and_then(|rules| rules.values.get(key)) else {
        return false;
    };
    match key {
        "traffic.reset_day" => parsed_vps_rule_json_integer(rule, key, "day")
            .filter(|value| (1..=31).contains(value))
            .zip(
                expected_vps_rule_json_integer(key, expected, "day")
                    .filter(|value| (1..=31).contains(value)),
            )
            .is_some_and(|(actual, expected)| compare_order(actual, expected, operator)),
        "traffic.quota.total" | "traffic.quota.rx" | "traffic.quota.tx" => {
            parsed_vps_rule_json_integer(rule, key, "bytes")
                .filter(|value| *value >= 0)
                .zip(
                    expected_vps_rule_json_integer(key, expected, "bytes")
                        .filter(|value| *value >= 0),
                )
                .is_some_and(|(actual, expected)| compare_order(actual, expected, operator))
        }
        "network.port_speed" => parsed_vps_rule_json_integer(rule, key, "bps")
            .filter(|value| *value > 0)
            .zip(expected_vps_rule_json_integer(key, expected, "bps").filter(|value| *value > 0))
            .is_some_and(|(actual, expected)| compare_order(actual, expected, operator)),
        "billing.price" => billing_price_from_rule(rule)
            .zip(parse_billing_price_order_value(expected))
            .filter(|(actual, expected)| {
                actual.currency == expected.currency && actual.period == expected.period
            })
            .is_some_and(|(actual, expected)| {
                compare_order(actual.amount_minor, expected.amount_minor, operator)
            }),
        _ => false,
    }
}

fn vps_rule_json_integer(value: &Value, field: &str) -> Option<i128> {
    let value = value.get(field)?;
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
        .or_else(|| value.as_str()?.parse::<i128>().ok())
}

fn parsed_vps_rule_json_integer(rule: &VpsRuleValue, key: &str, field: &str) -> Option<i128> {
    let parsed = parse_vps_rule_value(key, &rule.value_raw).ok()?;
    vps_rule_json_integer(&parsed.json, field)
}

fn expected_vps_rule_json_integer(key: &str, value: &str, field: &str) -> Option<i128> {
    let parsed = parse_vps_rule_value(key, value).ok()?;
    vps_rule_json_integer(&parsed.json, field)
}

fn parse_billing_price_order_value(value: &str) -> Option<BillingPriceOrderValue> {
    let parsed = parse_vps_rule_value("billing.price", value).ok()?;
    billing_price_from_json(&parsed.json, false)
}

fn billing_price_from_rule(rule: &VpsRuleValue) -> Option<BillingPriceOrderValue> {
    let parsed = parse_vps_rule_value("billing.price", &rule.value_raw).ok()?;
    billing_price_from_json(&parsed.json, true)
}

fn billing_price_from_json(value: &Value, allow_zero: bool) -> Option<BillingPriceOrderValue> {
    if value
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let amount_minor = parse_billing_amount_minor(value.get("price")?.as_str()?)?;
    if !allow_zero && amount_minor == 0 {
        return None;
    }
    Some(BillingPriceOrderValue {
        amount_minor,
        currency: value.get("currency")?.as_str()?.to_ascii_uppercase(),
        period: value.get("period_code")?.as_str()?.to_ascii_lowercase(),
    })
}

fn parse_billing_amount_minor(value: &str) -> Option<i128> {
    let (whole, fraction) = value.split_once('.')?;
    if whole.is_empty()
        || !whole.bytes().all(|digit| digit.is_ascii_digit())
        || fraction.len() != 2
        || !fraction.bytes().all(|digit| digit.is_ascii_digit())
    {
        return None;
    }
    whole
        .parse::<i128>()
        .ok()?
        .checked_mul(100)?
        .checked_add(fraction.parse::<i128>().ok()?)
}

fn compare_order<T: Ord>(actual: T, expected: T, operator: ComparisonOperator) -> bool {
    match operator {
        ComparisonOperator::Lt => actual < expected,
        ComparisonOperator::Lte => actual <= expected,
        ComparisonOperator::Gt => actual > expected,
        ComparisonOperator::Gte => actual >= expected,
        ComparisonOperator::Eq | ComparisonOperator::NotEq => unreachable!("not an order operator"),
    }
}

fn field_value_timestamp(value: &FieldValue) -> Option<i64> {
    match value {
        FieldValue::String(value) => parse_timestamp(value),
        FieldValue::Number(value) => i64::try_from(*value).ok(),
        FieldValue::Decimal(value) => value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && value.fract() == 0.0)
            .and_then(|value| {
                if value >= i64::MIN as f64 && value <= i64::MAX as f64 {
                    Some(value as i64)
                } else {
                    None
                }
            }),
    }
}

fn parse_timestamp(value: &str) -> Option<i64> {
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(seconds);
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc).timestamp())
}

fn list_values_match(actual: &str, expected_values: &[ListValue]) -> bool {
    expected_values.iter().any(|expected| match expected {
        ListValue::Literal(expected) => literal_matches(actual, expected, false),
        ListValue::Regex(pattern) => Regex::new(pattern).is_ok_and(|regex| regex.is_match(actual)),
    })
}

fn literal_matches(value: &str, pattern: &str, allow_contains: bool) -> bool {
    let value = value.to_ascii_lowercase();
    let pattern = pattern.to_ascii_lowercase();
    if pattern.contains('*') || pattern.contains('?') {
        glob_matches(&value, &pattern)
    } else if allow_contains {
        value.contains(&pattern)
    } else {
        value == pattern
    }
}

fn glob_matches(value: &str, pattern: &str) -> bool {
    let value = value.as_bytes();
    let pattern = pattern.as_bytes();
    let mut value_index = 0;
    let mut pattern_index = 0;
    let mut star_index: Option<usize> = None;
    let mut star_value_index = 0;
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            value_index += 1;
            pattern_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn field_values(context: &ExpressionContext, field: &str) -> Option<Vec<FieldValue>> {
    let field = canonical_field(field);
    if let Some(values) = vps_rule_field_values(context.vps_rules.as_ref(), &field) {
        return Some(values);
    }
    if let Some(values) = vps_field_values(context.vps.as_ref(), &field) {
        return Some(values);
    }
    let (root, path) = field.split_once('.')?;
    let value = match root {
        "rule" => context.rule.as_ref(),
        "event" => context.event.as_ref(),
        "query" => context.query.as_ref(),
        "server" => context.server.as_ref(),
        "job" => context.job.as_ref(),
        "schedule" => context.schedule.as_ref(),
        "alert" => context.alert.as_ref(),
        "policy_rule" => context.policy_rule.as_ref(),
        "telemetry" => context.telemetry.as_ref(),
        other => context.objects.get(other),
    }?;
    json_path_values(value, path)
}

fn vps_rule_field_values(rules: Option<&VpsRuleContext>, field: &str) -> Option<Vec<FieldValue>> {
    if field == VPS_RULE_FIELD {
        let rules = rules?;
        return Some(
            rules
                .values
                .keys()
                .cloned()
                .map(FieldValue::String)
                .collect(),
        );
    }
    let key = field.strip_prefix("vps.rules:")?;
    let rule = rules?.values.get(key)?;
    Some(vec![FieldValue::String(rule.value_raw.clone())])
}

fn collect_expression_roots(expression: &Expression, roots: &mut BTreeSet<String>) {
    match expression {
        Expression::Predicate(predicate) => collect_predicate_roots(predicate, roots),
        Expression::Not(inner) => collect_expression_roots(inner, roots),
        Expression::And(left, right) | Expression::Or(left, right) => {
            collect_expression_roots(left, roots);
            collect_expression_roots(right, roots);
        }
    }
}

fn collect_predicate_roots(predicate: &Predicate, roots: &mut BTreeSet<String>) {
    let field = match predicate {
        Predicate::Comparison { field, .. } | Predicate::Membership { field, .. } => field,
        Predicate::Bare(_) | Predicate::Event(_) | Predicate::Untagged => return,
    };
    if let Some((root, _path)) = field.split_once('.') {
        roots.insert(root.to_string());
    }
}

fn collect_expression_events(expression: &Expression, events: &mut BTreeSet<String>) {
    match expression {
        Expression::Predicate(Predicate::Event(event)) => {
            events.insert(event.clone());
        }
        Expression::Predicate(_) => {}
        Expression::Not(inner) => collect_expression_events(inner, events),
        Expression::And(left, right) | Expression::Or(left, right) => {
            collect_expression_events(left, events);
            collect_expression_events(right, events);
        }
    }
}

fn vps_field_values(vps: Option<&VpsMetadata>, field: &str) -> Option<Vec<FieldValue>> {
    let vps = vps?;
    match field {
        "vps.id" => Some(vec![FieldValue::String(vps.id.clone())]),
        "vps.display_name" => Some(vec![FieldValue::String(vps.display_name.clone())]),
        "vps.status" => Some(vec![FieldValue::String(vps.status.clone())]),
        "vps.tag" => Some(
            vps.tags
                .iter()
                .cloned()
                .map(FieldValue::String)
                .collect::<Vec<_>>(),
        ),
        "vps.provider" => Some(tag_alias_values(&vps.tags, "provider:")),
        "vps.country" => Some(tag_alias_values(&vps.tags, "country:")),
        "vps.region" => Some(tag_alias_values(&vps.tags, "region:")),
        "vps.registration_ip" => option_string_value(&vps.registration_ip),
        "vps.last_ip" => option_string_value(&vps.last_ip),
        "vps.last_seen_at" => option_string_value(&vps.last_seen_at),
        "vps.internal_build_number" => vps
            .internal_build_number
            .map(|value| vec![FieldValue::Number(i128::from(value))]),
        "vps.stale_since" => option_string_value(&vps.stale_since),
        "vps.stale_reason" => option_string_value(&vps.stale_reason),
        _ if field.starts_with("vps.") => vps
            .extra
            .as_ref()
            .and_then(|extra| json_path_values(extra, field.trim_start_matches("vps."))),
        _ => None,
    }
}

fn tag_alias_values(tags: &[String], prefix: &str) -> Vec<FieldValue> {
    let prefix_lower = prefix.to_ascii_lowercase();
    tags.iter()
        .filter_map(|tag| {
            if tag.to_ascii_lowercase().starts_with(&prefix_lower) {
                Some(FieldValue::String(tag[prefix.len()..].to_string()))
            } else {
                None
            }
        })
        .collect()
}

fn option_string_value(value: &Option<String>) -> Option<Vec<FieldValue>> {
    value
        .as_ref()
        .map(|value| vec![FieldValue::String(value.clone())])
}

fn json_path_values(value: &Value, path: &str) -> Option<Vec<FieldValue>> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = current.get(segment)?;
    }
    json_value_to_field_values(current)
}

fn json_value_to_field_values(value: &Value) -> Option<Vec<FieldValue>> {
    match value {
        Value::String(value) => Some(vec![FieldValue::String(value.clone())]),
        Value::Number(value) => value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
            .map(|value| vec![FieldValue::Number(value)])
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .map(|_| vec![FieldValue::Decimal(value.to_string())])
            }),
        Value::Bool(value) => Some(vec![FieldValue::String(value.to_string())]),
        Value::Array(values) => {
            let flattened = values
                .iter()
                .filter_map(json_value_to_field_values)
                .flatten()
                .collect::<Vec<_>>();
            Some(flattened)
        }
        Value::Null | Value::Object(_) => None,
    }
}

#[cfg(test)]
#[path = "tests_expression.rs"]
mod tests;
