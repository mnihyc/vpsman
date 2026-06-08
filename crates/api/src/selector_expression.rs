use crate::model::AgentView;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectorExpr {
    Term(SelectorTerm),
    And(Box<SelectorExpr>, Box<SelectorExpr>),
    Or(Box<SelectorExpr>, Box<SelectorExpr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectorTerm {
    pub(crate) namespace: Option<String>,
    pub(crate) value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    And,
    LeftParen,
    Or,
    RightParen,
    Term(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
}

pub(crate) fn parse_selector_expression(input: &str) -> Result<Option<SelectorExpr>, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Ok(None);
    }
    let mut parser = Parser {
        tokens,
        position: 0,
    };
    let expression = parser.parse_or()?;
    if parser.peek().is_some() {
        return Err("unexpected token after expression".to_string());
    }
    Ok(Some(expression))
}

pub(crate) fn agent_matches_selector_expression(
    agent: &AgentView,
    expression: &SelectorExpr,
) -> bool {
    match expression {
        SelectorExpr::Term(term) => agent_matches_term(agent, term),
        SelectorExpr::And(left, right) => {
            agent_matches_selector_expression(agent, left)
                && agent_matches_selector_expression(agent, right)
        }
        SelectorExpr::Or(left, right) => {
            agent_matches_selector_expression(agent, left)
                || agent_matches_selector_expression(agent, right)
        }
    }
}

pub(crate) fn id_selector_expression(client_id: &str) -> String {
    format!("id:{}", client_id.trim())
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars = input.char_indices().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        let (_, character) = chars[index];
        if character.is_whitespace() {
            index += 1;
            continue;
        }
        if character == '(' {
            tokens.push(Token {
                kind: TokenKind::LeftParen,
            });
            index += 1;
            continue;
        }
        if character == ')' {
            tokens.push(Token {
                kind: TokenKind::RightParen,
            });
            index += 1;
            continue;
        }
        if character == '&' && chars.get(index + 1).is_some_and(|(_, next)| *next == '&') {
            tokens.push(Token {
                kind: TokenKind::And,
            });
            index += 2;
            continue;
        }
        if character == '|' && chars.get(index + 1).is_some_and(|(_, next)| *next == '|') {
            tokens.push(Token {
                kind: TokenKind::Or,
            });
            index += 2;
            continue;
        }
        if character == '&' || character == '|' {
            return Err("use && or || for boolean operators".to_string());
        }
        let start = chars[index].0;
        let mut end = input.len();
        let mut cursor = index;
        while cursor < chars.len() {
            let (byte_index, current) = chars[cursor];
            if current.is_whitespace() || matches!(current, '(' | ')' | '&' | '|') {
                end = byte_index;
                break;
            }
            cursor += 1;
        }
        let raw = input[start..end].trim();
        if raw.is_empty() {
            index = cursor;
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        let kind = match lower.as_str() {
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            _ => TokenKind::Term(raw.to_string()),
        };
        tokens.push(Token { kind });
        index = cursor;
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn parse_or(&mut self) -> Result<SelectorExpr, String> {
        let mut expression = self.parse_and()?;
        while self.consume_or() {
            let right = self.parse_and()?;
            expression = SelectorExpr::Or(Box::new(expression), Box::new(right));
        }
        Ok(expression)
    }

    fn parse_and(&mut self) -> Result<SelectorExpr, String> {
        let mut expression = self.parse_primary()?;
        loop {
            if self.consume_and() {
                let right = self.parse_primary()?;
                expression = SelectorExpr::And(Box::new(expression), Box::new(right));
                continue;
            }
            if self.next_starts_primary() {
                let right = self.parse_primary()?;
                expression = SelectorExpr::And(Box::new(expression), Box::new(right));
                continue;
            }
            break;
        }
        Ok(expression)
    }

    fn parse_primary(&mut self) -> Result<SelectorExpr, String> {
        match self.advance() {
            Some(TokenKind::Term(raw)) => parse_term(raw).map(SelectorExpr::Term),
            Some(TokenKind::LeftParen) => {
                let expression = self.parse_or()?;
                if !matches!(self.advance(), Some(TokenKind::RightParen)) {
                    return Err("missing closing parenthesis".to_string());
                }
                Ok(expression)
            }
            Some(TokenKind::And | TokenKind::Or) => {
                Err("operator is missing a left operand".to_string())
            }
            Some(TokenKind::RightParen) => Err("unexpected closing parenthesis".to_string()),
            None => Err("expression is incomplete".to_string()),
        }
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

    fn next_starts_primary(&self) -> bool {
        matches!(
            self.peek(),
            Some(TokenKind::Term(_)) | Some(TokenKind::LeftParen)
        )
    }

    fn advance(&mut self) -> Option<TokenKind> {
        let token = self.tokens.get(self.position)?.kind.clone();
        self.position += 1;
        Some(token)
    }

    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.position).map(|token| &token.kind)
    }
}

fn parse_term(raw: String) -> Result<SelectorTerm, String> {
    if let Some(separator) = raw.find(':') {
        if separator == 0 {
            return Err("selector namespace is empty".to_string());
        }
        if separator == raw.len() - 1 {
            return Err("selector value is empty".to_string());
        }
        return Ok(SelectorTerm {
            namespace: Some(raw[..separator].to_ascii_lowercase()),
            value: raw[separator + 1..].to_string(),
        });
    }
    Ok(SelectorTerm {
        namespace: None,
        value: raw,
    })
}

fn agent_matches_term(agent: &AgentView, term: &SelectorTerm) -> bool {
    match term.namespace.as_deref() {
        Some("id") => value_matches(&agent.id, &term.value, false),
        Some("name") => value_matches(&agent.display_name, &term.value, false),
        Some("tag") => agent
            .tags
            .iter()
            .any(|tag| value_matches(tag, &term.value, false)),
        Some("provider") => agent
            .tags
            .iter()
            .any(|tag| value_matches(tag, &format!("provider:{}", term.value), false)),
        Some("country") | Some("region") => agent
            .tags
            .iter()
            .any(|tag| value_matches(tag, &format!("country:{}", term.value), false)),
        Some("status") => value_matches(&agent.status, &term.value, false),
        Some(_) => false,
        None => {
            value_matches(&agent.id, &term.value, true)
                || value_matches(&agent.display_name, &term.value, true)
        }
    }
}

fn value_matches(value: &str, pattern: &str, allow_contains: bool) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use vpsman_common::AgentCapabilitySnapshot;

    fn agent(id: &str, name: &str, tags: &[&str]) -> AgentView {
        AgentView {
            id: id.to_string(),
            display_name: name.to_string(),
            status: "online".to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        }
    }

    #[test]
    fn parses_boolean_expression_with_implicit_and_and_globs() {
        let expression =
            parse_selector_expression("(provider:alpha || provider:beta) country:U? id:edge-*")
                .unwrap()
                .unwrap();
        assert!(agent_matches_selector_expression(
            &agent("edge-01", "Edge One", &["provider:alpha", "country:US"]),
            &expression,
        ));
        assert!(!agent_matches_selector_expression(
            &agent("edge-01", "Edge One", &["provider:gamma", "country:US"]),
            &expression,
        ));
    }

    #[test]
    fn bare_terms_match_id_or_name_by_contains() {
        let expression = parse_selector_expression("fra").unwrap().unwrap();
        assert!(agent_matches_selector_expression(
            &agent("agent-fra-02", "core-fra-02", &["country:DE"]),
            &expression,
        ));
        assert!(!agent_matches_selector_expression(
            &agent("agent-sfo-01", "edge-sfo-01", &["country:US"]),
            &expression,
        ));
    }

    #[test]
    fn boolean_tags_match_independent_tag_entries() {
        let expression = parse_selector_expression("tag:edge && tag:country:US")
            .unwrap()
            .unwrap();
        assert!(agent_matches_selector_expression(
            &agent(
                "agent-sfo-01",
                "edge-sfo-01",
                &["edge", "provider:alpha", "country:US"],
            ),
            &expression,
        ));
        assert!(!agent_matches_selector_expression(
            &agent("agent-sfo-01", "edge-sfo-01", &["edge"]),
            &expression,
        ));
    }

    #[test]
    fn invalid_expressions_report_errors() {
        assert!(parse_selector_expression("(provider:alpha").is_err());
        assert!(parse_selector_expression("provider:").is_err());
        assert!(parse_selector_expression("provider:alpha ||").is_err());
    }
}
