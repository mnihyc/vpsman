use std::time::{SystemTime, UNIX_EPOCH};

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

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::search_pattern;

    #[test]
    fn search_pattern_escapes_like_wildcards() {
        assert_eq!(
            search_pattern(&Some(r"edge_%\host".to_string())),
            Some(r"%edge\_\%\\host%".to_string())
        );
        assert_eq!(search_pattern(&Some("   ".to_string())), None);
    }
}
