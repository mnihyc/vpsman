use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

pub(crate) fn read_json_file<T: serde::de::DeserializeOwned>(
    path: Option<&PathBuf>,
) -> Result<Option<T>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read JSON file {}", path.display()))?;
    Ok(Some(serde_json::from_str(&content).with_context(|| {
        format!("failed to parse JSON file {}", path.display())
    })?))
}

pub(crate) fn ensure_payload_hash(value: &str) -> Result<()> {
    anyhow::ensure!(
        value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "--payload-hash-hex must be a 64-character SHA-256 hex digest"
    );
    Ok(())
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn percent_encode_query_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

pub(crate) fn percent_encode_path_segment(value: &str) -> String {
    percent_encode_query_value(value)
}

#[cfg(test)]
mod tests {
    use super::{percent_encode_path_segment, percent_encode_query_value};

    #[test]
    fn percent_encodes_query_values_without_touching_safe_bytes() {
        assert_eq!(percent_encode_query_value("agent-a_1"), "agent-a_1");
        assert_eq!(percent_encode_query_value("agent a/b"), "agent%20a%2Fb");
        assert_eq!(percent_encode_path_segment("agent a/b"), "agent%20a%2Fb");
    }
}
