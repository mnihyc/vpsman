use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{Postgres, QueryBuilder};
use vpsman_common::{
    parse_field_expression, ComparisonOperator, Expression, ListValue, Predicate, ScalarValue,
    JOB_STATUSES, JOB_TARGET_STATUSES,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Scope {
    Job,
    Target,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SearchField {
    pub(crate) name: &'static str,
    pub(crate) aliases: Vec<&'static str>,
    pub(crate) kind: &'static str,
    pub(crate) description: &'static str,
    pub(crate) operators: Vec<&'static str>,
    pub(crate) examples: Vec<&'static str>,
    pub(crate) values: Vec<String>,
    pub(crate) scope: &'static str,
    #[serde(skip)]
    sql: &'static str,
}

pub(crate) fn search_fields() -> Vec<SearchField> {
    let mut fields = Vec::new();
    let mut add = |scope,
                   name,
                   aliases: &[&'static str],
                   kind,
                   sql,
                   description,
                   examples: &[&'static str]| {
        fields.push(SearchField {
            name,
            aliases: aliases.to_vec(),
            kind,
            sql,
            description,
            scope,
            operators: if matches!(kind, "integer" | "duration" | "timestamp") {
                vec!["=", "!=", "<", "<=", ">", ">=", "in", "not in"]
            } else if kind == "expression" {
                vec!["=", "!="]
            } else {
                vec!["=", "!=", "in", "not in"]
            },
            examples: examples.to_vec(),
            values: Vec::new(),
        });
    };
    add(
        "job",
        "type",
        &["command_type", "operation"],
        "text",
        "j.command_type",
        "Stored operation type; * and ? are wildcards.",
        &["runtime_config_sync", "network_routing_*"],
    );
    add(
        "job",
        "status",
        &["result"],
        "status",
        "j.status",
        "Overall job status.",
        &["failed", "completed"],
    );
    add(
        "job",
        "target",
        &["client_id"],
        "target",
        "",
        "Any recorded target ID or its current VPS name, including historical target IDs.",
        &["v-11"],
    );
    add("job", "target_result", &[], "expression", "", "A quoted expression evaluated against ONE target. Example: target_result = \"id = v-11 && status = failed\". != means no target matches.", &["id = v-11 && status = failed"]);
    add(
        "job",
        "id",
        &["job_id"],
        "text",
        "j.id::text",
        "Job ID.",
        &[],
    );
    add(
        "job",
        "target_count",
        &["targets"],
        "integer",
        "j.target_count",
        "Number of recorded job targets.",
        &["1", "10"],
    );
    add(
        "job",
        "actor_id",
        &[],
        "text",
        "j.actor_id::text",
        "Operator ID; = null finds jobs without an operator.",
        &["null"],
    );
    add(
        "job",
        "actor",
        &["started_by"],
        "text",
        "actor.username",
        "Current operator username. Use source = automation for worker jobs.",
        &[],
    );
    add("job", "source", &[], "source", "CASE WHEN j.source_schedule_id IS NOT NULL THEN 'schedule' WHEN j.actor_id IS NULL THEN 'automation' ELSE 'operator' END", "schedule, automation (no operator or schedule), or operator.", &["automation", "schedule", "operator"]);
    add(
        "job",
        "privileged",
        &[],
        "boolean",
        "j.privileged",
        "Whether this job required privileged authorization.",
        &["true", "false"],
    );
    add(
        "job",
        "schedule_id",
        &["source_schedule_id"],
        "text",
        "j.source_schedule_id::text",
        "Originating schedule ID; = null finds jobs without a schedule.",
        &["null"],
    );
    add(
        "job",
        "schedule",
        &[],
        "text",
        "schedule.name",
        "Current name of the originating schedule.",
        &[],
    );
    add(
        "job",
        "causation_id",
        &[],
        "text",
        "j.causation_id::text",
        "Recorded causation ID.",
        &[],
    );
    add(
        "job",
        "schedule_lineage",
        &[],
        "lineage",
        "lineage.value::text",
        "Any schedule ID in the recorded lineage. != means no ID matches.",
        &[],
    );
    add(
        "job",
        "payload_hash",
        &["hash"],
        "text",
        "j.payload_hash",
        "Stored payload hash.",
        &[],
    );
    add(
        "job",
        "created_at",
        &["created", "started"],
        "timestamp",
        "j.created_at",
        "Job creation time. Use an ISO timestamp with offset, a UTC date, now, or now-24h.",
        &["now-24h", "2026-01-01"],
    );
    add(
        "job",
        "completed_at",
        &["completed"],
        "timestamp",
        "j.completed_at",
        "Completion time; = null selects incomplete jobs.",
        &["now-24h", "null"],
    );
    add("job", "duration", &[], "duration", "CASE WHEN j.completed_at IS NOT NULL THEN GREATEST(0, EXTRACT(EPOCH FROM (j.completed_at-j.created_at))) END", "Completed minus created time, including queueing; incomplete is null. Units: ms, s, m, h, d, w; a bare number is seconds.", &["30s", "5m", "null"]);
    add("job", "age", &[], "duration", "", "Time since creation at the query's reference time; fixed while paging. Units: ms, s, m, h, d, w.", &["1h", "7d"]);
    add(
        "job",
        "timeout",
        &["max_timeout_secs"],
        "duration",
        "j.max_timeout_secs",
        "Configured timeout; bare numbers are seconds.",
        &["30s", "5m"],
    );
    add(
        "job",
        "resource_kind",
        &[],
        "text",
        "j.resource_kind",
        "Stored resource kind, where present.",
        &["file_transfer_session", "null"],
    );
    add(
        "job",
        "resource_id",
        &[],
        "text",
        "j.resource_id::text",
        "Stored resource ID, where present.",
        &[],
    );
    add(
        "target",
        "id",
        &["client_id", "target"],
        "text",
        "t.client_id",
        "Recorded VPS ID.",
        &["v-11"],
    );
    add(
        "target",
        "name",
        &[],
        "text",
        "c.display_name",
        "Current VPS name; historical IDs remain searchable if the VPS was removed.",
        &[],
    );
    add(
        "target",
        "status",
        &[],
        "status",
        "t.status",
        "This target's status, independent of the job's overall status.",
        &["failed", "agent_lost"],
    );
    add(
        "target",
        "exit_code",
        &[],
        "integer",
        "t.exit_code",
        "This target's exit code; unavailable is null.",
        &["0", "1", "null"],
    );
    add(
        "target",
        "message",
        &[],
        "text",
        "t.message",
        "Target result message; use *text* to find a substring.",
        &["*timeout*"],
    );
    add(
        "target",
        "started_at",
        &["started"],
        "timestamp",
        "t.started_at",
        "Target execution start time; not yet started is null.",
        &["now-24h", "null"],
    );
    add(
        "target",
        "completed_at",
        &["completed"],
        "timestamp",
        "t.completed_at",
        "Target completion time.",
        &["now-24h", "null"],
    );
    add(
        "target",
        "deadline_at",
        &["deadline"],
        "timestamp",
        "t.deadline_at",
        "Target execution deadline.",
        &["now", "null"],
    );
    add("target", "duration", &[], "duration", "CASE WHEN t.completed_at IS NOT NULL AND t.started_at IS NOT NULL THEN GREATEST(0, EXTRACT(EPOCH FROM (t.completed_at-t.started_at))) END", "Target execution time; requires both start and completion.", &["30s", "null"]);
    add(
        "target",
        "dispatch_attempts",
        &[],
        "integer",
        "t.dispatch_attempts",
        "Number of dispatch attempts.",
        &["1"],
    );
    add(
        "target",
        "last_dispatch_error",
        &[],
        "text",
        "t.last_dispatch_error",
        "Last stored dispatch error.",
        &["*timeout*"],
    );
    add(
        "target",
        "capability_reason",
        &["capability_degraded_reason"],
        "text",
        "t.capability_degraded_reason",
        "Recorded capability degradation reason.",
        &["null"],
    );
    for field in &mut fields {
        field.values = match (field.kind, field.scope) {
            ("status", "job") => JOB_STATUSES.iter().map(|v| (*v).into()).collect(),
            ("status", _) => JOB_TARGET_STATUSES.iter().map(|v| (*v).into()).collect(),
            ("boolean", _) => vec!["true".into(), "false".into()],
            ("source", _) => vec!["automation".into(), "schedule".into(), "operator".into()],
            _ => Vec::new(),
        };
        if field.name == "type" && field.scope == "job" {
            field.values = vpsman_common::job_command_type_labels()
                .iter()
                .map(|name| (*name).to_string())
                .collect();
            field.values.sort();
            field.values.dedup();
        }
    }
    fields
}

#[derive(Clone, Debug)]
pub(super) enum Filter {
    All,
    Bare(String, Scope),
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    Not(Box<Self>),
    Target(Box<Self>),
    Compare(SearchField, ComparisonOperator, Value),
    Regex(SearchField, String),
}

#[derive(Clone, Debug)]
pub(super) enum Value {
    Text(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    Time(DateTime<Utc>),
    Null,
}

pub(super) fn parse(input: &str, now: DateTime<Utc>) -> Result<Filter, String> {
    // Bound SQL expansion, not history depth. 16 KiB supports long ID lists in
    // one operator query while rejecting accidental pasted command output.
    if input.len() > 16 * 1024 {
        return Err("Search expressions may contain at most 16 KiB.".into());
    }
    let mut budget = 256;
    parse_scoped(input, Scope::Job, now, &mut budget)
}

fn parse_scoped(
    input: &str,
    scope: Scope,
    now: DateTime<Utc>,
    budget: &mut usize,
) -> Result<Filter, String> {
    let Some(expression) = parse_field_expression(input)? else {
        return Ok(Filter::All);
    };
    convert(expression, scope, now, budget)
}

fn convert(
    expression: Expression,
    scope: Scope,
    now: DateTime<Utc>,
    budget: &mut usize,
) -> Result<Filter, String> {
    *budget = budget
        .checked_sub(1)
        .ok_or("Search has too many conditions (maximum 256 expression nodes).")?;
    Ok(match expression {
        Expression::And(a, b) => Filter::And(
            Box::new(convert(*a, scope, now, budget)?),
            Box::new(convert(*b, scope, now, budget)?),
        ),
        Expression::Or(a, b) => Filter::Or(
            Box::new(convert(*a, scope, now, budget)?),
            Box::new(convert(*b, scope, now, budget)?),
        ),
        Expression::Not(a) => Filter::Not(Box::new(convert(*a, scope, now, budget)?)),
        Expression::Predicate(Predicate::Bare(value)) if value == "*" => Filter::All,
        Expression::Predicate(Predicate::Bare(value)) => Filter::Bare(value, scope),
        Expression::Predicate(Predicate::Comparison {
            field,
            operator,
            value: ScalarValue::Literal(value),
        }) => {
            let field = find_field(&field, scope)?;
            if field.kind == "expression" {
                if !matches!(operator, ComparisonOperator::Eq | ComparisonOperator::NotEq) {
                    return Err(
                        "target_result supports only = or != with a quoted target expression."
                            .into(),
                    );
                }
                let inner =
                    Filter::Target(Box::new(parse_scoped(&value, Scope::Target, now, budget)?));
                if operator == ComparisonOperator::NotEq {
                    Filter::Not(Box::new(inner))
                } else {
                    inner
                }
            } else {
                comparison(field, operator, &value, now)?
            }
        }
        Expression::Predicate(Predicate::Membership {
            field,
            negated,
            values,
        }) => {
            let field = find_field(&field, scope)?;
            if field.kind == "expression" {
                return Err("target_result uses = with a quoted target expression.".into());
            }
            let mut filters = Vec::new();
            for value in values {
                *budget = budget
                    .checked_sub(1)
                    .ok_or("Search has too many list values.")?;
                filters.push(match value {
                    ListValue::Literal(value) => {
                        comparison(field.clone(), ComparisonOperator::Eq, &value, now)?
                    }
                    ListValue::Regex(pattern) if is_text(&field) => {
                        Filter::Regex(field.clone(), pattern)
                    }
                    ListValue::Regex(_) => {
                        return Err(format!(
                            "{} does not support regular expressions.",
                            field.name
                        ))
                    }
                });
            }
            let inner = filters
                .into_iter()
                .reduce(|a, b| Filter::Or(Box::new(a), Box::new(b)))
                .ok_or("Lists must contain at least one value.")?;
            if negated {
                Filter::Not(Box::new(inner))
            } else {
                inner
            }
        }
        Expression::Predicate(_) => {
            return Err("This selector is not a Job history search field.".into())
        }
    })
}

fn find_field(name: &str, scope: Scope) -> Result<SearchField, String> {
    let name = name.to_ascii_lowercase();
    let name = name
        .strip_prefix("job.")
        .filter(|_| scope == Scope::Job)
        .unwrap_or(&name);
    search_fields()
        .into_iter()
        .find(|f| {
            f.scope == if scope == Scope::Job { "job" } else { "target" }
                && (f.name == name || f.aliases.contains(&name))
        })
        .ok_or_else(|| {
            format!(
                "Unknown {} field '{name}'. Choose a field from the search hints.",
                if scope == Scope::Job { "job" } else { "target" }
            )
        })
}

fn is_text(field: &SearchField) -> bool {
    matches!(
        field.kind,
        "text" | "status" | "source" | "target" | "lineage"
    )
}

fn comparison(
    field: SearchField,
    op: ComparisonOperator,
    raw: &str,
    now: DateTime<Utc>,
) -> Result<Filter, String> {
    let ordered = !matches!(op, ComparisonOperator::Eq | ComparisonOperator::NotEq);
    if ordered && !matches!(field.kind, "integer" | "duration" | "timestamp") {
        return Err(format!("{} supports =, !=, in and not in.", field.name));
    }
    let value = if raw.eq_ignore_ascii_case("null") {
        if ordered {
            return Err("Use = null or != null for missing values.".into());
        }
        if matches!(field.kind, "target" | "lineage") {
            return Err(format!(
                "{} is a collection; use target_count or a negated membership expression.",
                field.name
            ));
        }
        Value::Null
    } else {
        match field.kind {
            "integer" => Value::Integer(
                raw.parse()
                    .map_err(|_| format!("{} expects a whole number.", field.name))?,
            ),
            "duration" => Value::Number(duration_seconds(raw)?),
            "timestamp" => Value::Time(timestamp(raw, now)?),
            "boolean" => Value::Boolean(match raw.to_ascii_lowercase().as_str() {
                "true" => true,
                "false" => false,
                _ => return Err(format!("{} expects true or false.", field.name)),
            }),
            _ => {
                if matches!(field.kind, "status" | "source")
                    && !raw.contains(['*', '?'])
                    && !field.values.iter().any(|v| v.eq_ignore_ascii_case(raw))
                {
                    return Err(format!("Unknown {} value '{raw}'.", field.name));
                }
                Value::Text(raw.into())
            }
        }
    };
    Ok(Filter::Compare(field, op, value))
}

pub(super) fn duration_seconds(raw: &str) -> Result<f64, String> {
    let split = raw
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(raw.len());
    let number: f64 = raw[..split]
        .parse()
        .map_err(|_| "Expected a duration such as 30s, 5m or 1h.")?;
    let factor = match raw[split..].to_ascii_lowercase().as_str() {
        "" | "s" => 1.0,
        "ms" => 0.001,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        "w" => 604800.0,
        _ => return Err("Duration units are ms, s, m, h, d or w.".into()),
    };
    let value = number * factor;
    if !value.is_finite() || value < 0.0 {
        return Err("Duration must be a finite, nonnegative number.".into());
    }
    Ok(value)
}

fn timestamp(raw: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    if raw == "now" {
        return Ok(now);
    }
    if let Some(duration) = raw.strip_prefix("now-") {
        let millis = duration_seconds(duration)? * 1000.0;
        if millis > i64::MAX as f64 {
            return Err("Relative time is out of range.".into());
        }
        return now
            .checked_sub_signed(chrono::Duration::milliseconds(millis as i64))
            .ok_or("Relative time is out of range.".into());
    }
    DateTime::parse_from_rfc3339(raw)
        .map(|t| t.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
        })
        .map_err(|_| {
            "Expected an ISO timestamp with a timezone, a UTC date, now, or now-24h.".into()
        })
}

pub(super) fn like_pattern(value: &str, contains: bool) -> String {
    let mut result = String::new();
    if contains {
        result.push('%');
    }
    for c in value.chars() {
        match c {
            '*' => result.push('%'),
            '?' => result.push('_'),
            '%' | '_' | '\\' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    if contains {
        result.push('%');
    }
    result
}

impl Filter {
    pub(super) fn uses_field(&self, name: &str) -> bool {
        match self {
            Self::Compare(field, ..) | Self::Regex(field, _) => {
                field.scope == "job" && field.name == name
            }
            Self::And(a, b) | Self::Or(a, b) => a.uses_field(name) || b.uses_field(name),
            Self::Not(a) | Self::Target(a) => a.uses_field(name),
            _ => false,
        }
    }

    pub(super) fn regexes(&self, output: &mut Vec<String>) {
        match self {
            Self::Regex(_, pattern) => output.push(pattern.clone()),
            Self::And(a, b) | Self::Or(a, b) => {
                a.regexes(output);
                b.regexes(output);
            }
            Self::Not(a) | Self::Target(a) => a.regexes(output),
            _ => {}
        }
    }

    pub(super) fn push(&self, sql: &mut QueryBuilder<'static, Postgres>, now: DateTime<Utc>) {
        match self {
            Self::All => {
                sql.push("TRUE");
            }
            Self::And(a, b) | Self::Or(a, b) => {
                sql.push("(");
                a.push(sql, now);
                sql.push(if matches!(self, Self::And(..)) {
                    " AND "
                } else {
                    " OR "
                });
                b.push(sql, now);
                sql.push(")");
            }
            Self::Not(a) => {
                sql.push("(NOT (");
                a.push(sql, now);
                sql.push("))");
            }
            Self::Target(a) => {
                target_start(sql);
                a.push(sql, now);
                sql.push(")");
            }
            Self::Bare(raw, scope) => {
                sql.push("(");
                sql.push(if *scope == Scope::Job {
                    "concat_ws(' ',j.id,j.command_type,replace(j.command_type,'_',' '),j.status,j.actor_id,CASE WHEN j.actor_id IS NULL THEN 'worker automation' ELSE 'operator' END,CASE WHEN j.privileged THEN 'privileged' ELSE 'unprivileged' END,j.source_schedule_id,j.causation_id,j.payload_hash)"
                } else {"concat_ws(' ',t.client_id,c.display_name,t.status,t.message,t.exit_code,t.capability_degraded_reason)"});
                sql.push(" ILIKE ")
                    .push_bind(like_pattern(raw, !raw.contains(['*', '?'])))
                    .push(" ESCAPE '\\'");
                if *scope == Scope::Job {
                    sql.push(" OR ");
                    target_start(sql);
                    sql.push("concat_ws(' ',t.client_id,c.display_name) ILIKE ")
                        .push_bind(like_pattern(raw, !raw.contains(['*', '?'])))
                        .push(" ESCAPE '\\')");
                }
                sql.push(")");
            }
            Self::Compare(field, op, value) => {
                if field.kind == "target" {
                    let Value::Text(value) = value else {
                        unreachable!()
                    };
                    if *op == ComparisonOperator::NotEq {
                        sql.push("NOT (");
                    }
                    target_match(sql, value, false);
                    if *op == ComparisonOperator::NotEq {
                        sql.push(")");
                    }
                    return;
                }
                // Collection != means no member matches; it is not satisfied
                // merely because a different target/lineage member exists.
                let collection = matches!(field.kind, "target" | "lineage");
                if collection && *op == ComparisonOperator::NotEq {
                    sql.push("NOT (");
                }
                collection_start(sql, field);
                if let Value::Text(text) = value {
                    text_match(
                        sql,
                        field.sql,
                        text,
                        false,
                        *op == ComparisonOperator::NotEq && !collection,
                    );
                    if collection {
                        sql.push(")");
                    }
                    if collection && *op == ComparisonOperator::NotEq {
                        sql.push(")");
                    }
                    return;
                }
                push_field(sql, field, now);
                if matches!(value, Value::Null) {
                    sql.push(if *op == ComparisonOperator::Eq {
                        " IS NULL"
                    } else {
                        " IS NOT NULL"
                    });
                } else {
                    sql.push(match op {
                        ComparisonOperator::Eq => " = ",
                        ComparisonOperator::NotEq => " <> ",
                        ComparisonOperator::Lt => " < ",
                        ComparisonOperator::Lte => " <= ",
                        ComparisonOperator::Gt => " > ",
                        ComparisonOperator::Gte => " >= ",
                    });
                    match value {
                        Value::Integer(n) => {
                            sql.push_bind(*n);
                        }
                        Value::Number(n) => {
                            sql.push_bind(*n);
                        }
                        Value::Boolean(v) => {
                            sql.push_bind(*v);
                        }
                        Value::Time(t) => {
                            sql.push_bind(*t);
                        }
                        _ => unreachable!(),
                    }
                }
                if collection {
                    sql.push(")");
                }
                if collection && *op == ComparisonOperator::NotEq {
                    sql.push(")");
                }
            }
            Self::Regex(field, pattern) => {
                if field.kind == "target" {
                    target_match(sql, pattern, true);
                    return;
                }
                collection_start(sql, field);
                push_field(sql, field, now);
                sql.push(" ~ ").push_bind(pattern.clone());
                if matches!(field.kind, "target" | "lineage") {
                    sql.push(")");
                }
            }
        }
    }
}

fn target_start(sql: &mut QueryBuilder<'static, Postgres>) {
    sql.push("EXISTS (SELECT 1 FROM job_targets t LEFT JOIN clients c ON c.id=t.client_id WHERE t.job_id=j.id AND ");
}

fn collection_start(sql: &mut QueryBuilder<'static, Postgres>, field: &SearchField) {
    if field.kind == "lineage" {
        sql.push("EXISTS (SELECT 1 FROM unnest(j.schedule_lineage) lineage(value) WHERE ");
    }
}

fn push_field(sql: &mut QueryBuilder<'static, Postgres>, field: &SearchField, now: DateTime<Utc>) {
    match field.kind {
        "lineage" => {
            sql.push("lineage.value::text");
        }
        _ if field.name == "age" && field.scope == "job" => {
            sql.push("EXTRACT(EPOCH FROM (")
                .push_bind(now)
                .push(" - j.created_at))");
        }
        _ => {
            sql.push(field.sql);
        }
    }
}

fn text_match(
    sql: &mut QueryBuilder<'static, Postgres>,
    field: &str,
    value: &str,
    regex: bool,
    negative: bool,
) {
    if regex {
        sql.push(field).push(" ~ ").push_bind(value.to_string());
    } else if value.contains(['*', '?']) {
        sql.push(field)
            .push(if negative { " NOT ILIKE " } else { " ILIKE " })
            .push_bind(like_pattern(value, false))
            .push(" ESCAPE '\\'");
    } else {
        // Equality can use a folded-value index; LIKE wildcards are opt-in.
        sql.push("lower(")
            .push(field)
            .push(")")
            .push(if negative { " <> lower(" } else { " = lower(" })
            .push_bind(value.to_string())
            .push(")");
    }
}

fn target_match(sql: &mut QueryBuilder<'static, Postgres>, value: &str, regex: bool) {
    // Resolve matching targets as a set before joining jobs. The UNION keeps
    // an ID/name collision from duplicating jobs and allows historical lookup
    // without one dependent subquery for every retained execution.
    sql.push("j.id IN (SELECT t.job_id FROM job_targets t WHERE ");
    text_match(sql, "t.client_id", value, regex, false);
    sql.push(" UNION SELECT t.job_id FROM job_targets t JOIN clients c ON c.id=t.client_id WHERE ");
    text_match(sql, "c.display_name", value, regex, false);
    sql.push(")");
}
