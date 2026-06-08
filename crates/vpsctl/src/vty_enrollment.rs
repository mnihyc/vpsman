use anyhow::{bail, Result};

use crate::http::http_post_json;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VtyEnrollmentTokenCreate {
    ttl_secs: u64,
    purpose: String,
    client_id: Option<String>,
    confirmed_reenrollment: bool,
    preserve_existing_assignments: bool,
    default_tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VtyClientKeyRevoke {
    client_id: String,
    confirmed: bool,
    reason: Option<String>,
}

pub(crate) fn is_vty_enrollment_command(command: &str) -> bool {
    matches!(
        command.split_whitespace().next(),
        Some("enrollment-token-create" | "reenrollment-token-create" | "client-key-revoke")
    )
}

pub(crate) fn submit_vty_enrollment_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let Some(verb) = parts.first().copied() else {
        bail!("empty enrollment command");
    };
    match verb {
        "enrollment-token-create" | "reenrollment-token-create" => {
            let request = if verb == "enrollment-token-create" {
                parse_vty_enrollment_token_create(&parts[1..])?
            } else {
                parse_vty_reenrollment_token_create(&parts[1..])?
            };
            http_post_json(
                api_url,
                "/api/v1/enrollment-tokens",
                token,
                &serde_json::json!({
                    "ttl_secs": request.ttl_secs,
                    "purpose": request.purpose,
                    "allowed_client_id": request.client_id,
                    "confirmed_reenrollment": request.confirmed_reenrollment,
                    "preserve_existing_assignments": request.preserve_existing_assignments,
                    "default_tags": request.default_tags,
                }),
            )
        }
        "client-key-revoke" => {
            let request = parse_vty_client_key_revoke(&parts[1..])?;
            http_post_json(
                api_url,
                &format!(
                    "/api/v1/clients/{}/key-revocations",
                    encode_path(&request.client_id)
                ),
                token,
                &serde_json::json!({
                    "confirmed": request.confirmed,
                    "reason": request.reason,
                }),
            )
        }
        _ => bail!("unknown enrollment command"),
    }
}

fn parse_vty_enrollment_token_create(tokens: &[&str]) -> Result<VtyEnrollmentTokenCreate> {
    let mut request = VtyEnrollmentTokenCreate {
        ttl_secs: 1800,
        purpose: "provision".to_string(),
        client_id: None,
        confirmed_reenrollment: false,
        preserve_existing_assignments: true,
        default_tags: Vec::new(),
    };
    parse_common_flags(tokens, &mut request, false)?;
    Ok(request)
}

fn parse_vty_reenrollment_token_create(tokens: &[&str]) -> Result<VtyEnrollmentTokenCreate> {
    let mut request = VtyEnrollmentTokenCreate {
        ttl_secs: 1800,
        purpose: "rebuild_reenrollment".to_string(),
        client_id: None,
        confirmed_reenrollment: false,
        preserve_existing_assignments: true,
        default_tags: Vec::new(),
    };
    parse_common_flags(tokens, &mut request, true)?;
    if request.client_id.is_none() {
        bail!("reenrollment-token-create requires --client-id <id>");
    }
    if !request.confirmed_reenrollment {
        bail!("reenrollment-token-create requires --confirmed");
    }
    Ok(request)
}

fn parse_common_flags(
    tokens: &[&str],
    request: &mut VtyEnrollmentTokenCreate,
    allow_reenrollment_flags: bool,
) -> Result<()> {
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--ttl" | "--ttl-secs" => {
                request.ttl_secs = parse_next_u64(tokens, &mut index, "ttl_secs")?;
            }
            "--client-id" if allow_reenrollment_flags => {
                request.client_id = Some(parse_next(tokens, &mut index, "client_id")?.to_string());
            }
            "--tags" | "--default-tags" => {
                request.default_tags = parse_next(tokens, &mut index, "tags")?
                    .split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "--confirmed" if allow_reenrollment_flags => {
                request.confirmed_reenrollment = true;
                index += 1;
            }
            "--reset-existing-assignments" if allow_reenrollment_flags => {
                request.preserve_existing_assignments = false;
                index += 1;
            }
            other => bail!("unknown enrollment flag {other}"),
        }
    }
    Ok(())
}

fn parse_vty_client_key_revoke(tokens: &[&str]) -> Result<VtyClientKeyRevoke> {
    let mut request = VtyClientKeyRevoke {
        client_id: String::new(),
        confirmed: false,
        reason: None,
    };
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--client-id" => {
                request.client_id = parse_next(tokens, &mut index, "client_id")?.to_string();
            }
            "--reason" => {
                request.reason = Some(parse_next(tokens, &mut index, "reason")?.to_string());
            }
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            other => bail!("unknown client-key-revoke flag {other}"),
        }
    }
    if request.client_id.trim().is_empty() {
        bail!("client-key-revoke requires --client-id <id>");
    }
    if !request.confirmed {
        bail!("client-key-revoke requires --confirmed");
    }
    Ok(request)
}

fn encode_path(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn parse_next<'a>(tokens: &'a [&str], index: &mut usize, name: &str) -> Result<&'a str> {
    let Some(value) = tokens.get(*index + 1).copied() else {
        bail!("{name} value required");
    };
    *index += 2;
    Ok(value)
}

fn parse_next_u64(tokens: &[&str], index: &mut usize, name: &str) -> Result<u64> {
    let value = parse_next(tokens, index, name)?;
    value
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("{name} must be an integer: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_vty_client_key_revoke, parse_vty_enrollment_token_create,
        parse_vty_reenrollment_token_create,
    };

    #[test]
    fn parses_vty_reenrollment_token_create() {
        let request = parse_vty_reenrollment_token_create(&[
            "--client-id",
            "edge-a",
            "--ttl",
            "600",
            "--default-tags",
            "rebuilt,provider:alpha",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.ttl_secs, 600);
        assert_eq!(request.purpose, "rebuild_reenrollment");
        assert_eq!(request.client_id.as_deref(), Some("edge-a"));
        assert!(request.confirmed_reenrollment);
        assert_eq!(
            request.default_tags,
            vec!["rebuilt".to_string(), "provider:alpha".to_string()]
        );
    }

    #[test]
    fn rejects_unconfirmed_vty_reenrollment_token() {
        assert!(parse_vty_reenrollment_token_create(&["--client-id", "edge-a"]).is_err());
        assert!(parse_vty_enrollment_token_create(&["--client-id", "edge-a"]).is_err());
    }

    #[test]
    fn parses_vty_client_key_revoke() {
        let request = parse_vty_client_key_revoke(&[
            "--client-id",
            "edge-a",
            "--reason",
            "rebuilt",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.client_id, "edge-a");
        assert_eq!(request.reason.as_deref(), Some("rebuilt"));
        assert!(request.confirmed);
    }

    #[test]
    fn rejects_unconfirmed_vty_client_key_revoke() {
        assert!(parse_vty_client_key_revoke(&["--client-id", "edge-a"]).is_err());
    }
}
