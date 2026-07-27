use std::{
    cmp::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Utc};
use vpsman_common::OutputStream;

pub(crate) fn limit_or_default(limit: Option<i64>) -> i64 {
    limit.unwrap_or(100).clamp(1, 1000)
}

pub(crate) fn offset_or_default(offset: Option<i64>) -> i64 {
    offset.unwrap_or(0).clamp(0, 100_000)
}

pub(crate) fn sort_descending(dir: Option<&str>, default_descending: bool) -> bool {
    match dir.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "asc" => false,
        Some(value) if value == "desc" => true,
        _ => default_descending,
    }
}

pub(crate) fn search_pattern(q: &Option<String>) -> Option<String> {
    q.as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{}%", escape_like_pattern(value)))
}

fn escape_like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

pub(crate) fn output_stream_name(stream: OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
        OutputStream::Pty => "pty",
        OutputStream::Status => "status",
    }
}

pub(crate) fn parse_timestamp_utc(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(timestamp) = value.parse::<i64>() {
        return (timestamp >= 0)
            .then(|| DateTime::<Utc>::from_timestamp(timestamp, 0))
            .flatten();
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .or_else(|| DateTime::parse_from_rfc3339(&normalize_postgres_timestamp(value)).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .filter(|timestamp| timestamp.timestamp() >= 0)
}

pub(crate) fn parse_timestamp_unix(value: &str) -> Option<u64> {
    parse_timestamp_utc(value).map(|timestamp| timestamp.timestamp() as u64)
}

pub(crate) fn timestamp_in_optional_bounds(
    value: &str,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
) -> bool {
    if start_unix.is_none() && end_unix.is_none() {
        return true;
    }
    parse_timestamp_unix(value).is_some_and(|timestamp| {
        start_unix.is_none_or(|start| timestamp >= start)
            && end_unix.is_none_or(|end| timestamp <= end)
    })
}

pub(crate) fn compare_timestamps_desc(left: &str, right: &str) -> Ordering {
    match (parse_timestamp_utc(left), parse_timestamp_utc(right)) {
        (Some(left_timestamp), Some(right_timestamp)) => right_timestamp
            .cmp(&left_timestamp)
            .then_with(|| right.cmp(left)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => right.cmp(left),
    }
}

fn normalize_postgres_timestamp(value: &str) -> String {
    let mut normalized = value.replacen(' ', "T", 1);
    if let Some(offset_start) = normalized.rfind(['+', '-']) {
        let offset = &normalized[offset_start..];
        if offset.len() == 3 {
            normalized.push_str(":00");
        } else if offset.len() == 5 && !offset.contains(':') {
            normalized.insert(offset_start + 3, ':');
        }
    }
    normalized
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{
        compare_timestamps_desc, parse_timestamp_unix, parse_timestamp_utc, search_pattern,
        timestamp_in_optional_bounds,
    };
    use std::cmp::Ordering;

    #[test]
    fn search_pattern_escapes_like_wildcards() {
        assert_eq!(
            search_pattern(&Some(r"edge_%\host".to_string())),
            Some(r"%edge\_\%\\host%".to_string())
        );
        assert_eq!(search_pattern(&Some("   ".to_string())), None);
    }

    #[test]
    fn timestamp_helpers_compare_mixed_wire_formats_chronologically() {
        assert_eq!(parse_timestamp_unix("1970-01-01 00:02:00+00"), Some(120));
        assert_eq!(
            compare_timestamps_desc("120", "1970-01-01T00:01:00Z"),
            Ordering::Less
        );
        assert!(
            parse_timestamp_utc("1970-01-01T00:00:00.1Z")
                > parse_timestamp_utc("1970-01-01T00:00:00Z")
        );
        assert_eq!(
            parse_timestamp_utc("1970-01-01T01:00:00+01:00"),
            parse_timestamp_utc("1970-01-01T00:00:00Z")
        );
    }

    #[test]
    fn bounded_timestamp_checks_reject_malformed_values() {
        assert!(timestamp_in_optional_bounds("malformed", None, None));
        assert!(!timestamp_in_optional_bounds("malformed", Some(1), None));
        assert!(!timestamp_in_optional_bounds("malformed", None, Some(1)));
    }
}
