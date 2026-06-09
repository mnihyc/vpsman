use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::{
    commands_config::{agent_update_artifact_upload_response, validate_update_input},
    http::{http_get, http_post_json},
};

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateReleaseRecordRequest {
    name: String,
    version: String,
    channel: String,
    artifact_url: String,
    sha256_hex: String,
    artifact_signature_hex: String,
    artifact_signing_key_hex: String,
    rollback_artifact_file: Option<PathBuf>,
    rollback_artifact_url: Option<String>,
    rollback_signing_seed_hex: Option<String>,
    size_bytes: Option<i64>,
    notes: Option<String>,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateArtifactUploadRequest {
    name: String,
    version: String,
    channel: String,
    artifact_file: PathBuf,
    signing_seed_hex: String,
    rollback_artifact_file: Option<PathBuf>,
    rollback_signing_seed_hex: Option<String>,
    notes: Option<String>,
    stream: bool,
}

pub(crate) fn parse_vty_agent_update_release_record(
    tokens: &[&str],
) -> Result<VtyAgentUpdateReleaseRecordRequest> {
    let mut name = None;
    let mut version = None;
    let mut channel = "stable".to_string();
    let mut artifact_url = None;
    let mut sha256_hex = None;
    let mut artifact_signature_hex = None;
    let mut artifact_signing_key_hex = None;
    let mut rollback_artifact_file = None;
    let mut rollback_artifact_url = None;
    let mut rollback_signing_seed_hex = None;
    let mut size_bytes = None;
    let mut notes = None;
    let mut confirmed = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--name" => {
                name = Some(
                    tokens
                        .get(index + 1)
                        .context("--name requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--version" => {
                version = Some(
                    tokens
                        .get(index + 1)
                        .context("--version requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--channel" => {
                channel = tokens
                    .get(index + 1)
                    .context("--channel requires a value")?
                    .to_string();
                index += 2;
            }
            "--artifact-url" => {
                artifact_url = Some(
                    tokens
                        .get(index + 1)
                        .context("--artifact-url requires a URL")?
                        .to_string(),
                );
                index += 2;
            }
            "--sha256-hex" => {
                sha256_hex = Some(
                    tokens
                        .get(index + 1)
                        .context("--sha256-hex requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--artifact-signature-hex" => {
                artifact_signature_hex = Some(
                    tokens
                        .get(index + 1)
                        .context("--artifact-signature-hex requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--artifact-signing-key-hex" => {
                artifact_signing_key_hex = Some(
                    tokens
                        .get(index + 1)
                        .context("--artifact-signing-key-hex requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--rollback-artifact-file" => {
                rollback_artifact_file = Some(PathBuf::from(
                    tokens
                        .get(index + 1)
                        .context("--rollback-artifact-file requires a path")?,
                ));
                index += 2;
            }
            "--rollback-artifact-url" => {
                rollback_artifact_url = Some(
                    tokens
                        .get(index + 1)
                        .context("--rollback-artifact-url requires a URL")?
                        .to_string(),
                );
                index += 2;
            }
            "--rollback-signing-seed-hex" => {
                rollback_signing_seed_hex = Some(
                    tokens
                        .get(index + 1)
                        .context("--rollback-signing-seed-hex requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--size-bytes" => {
                size_bytes = Some(
                    tokens
                        .get(index + 1)
                        .context("--size-bytes requires a value")?
                        .parse()
                        .context("--size-bytes must be an integer")?,
                );
                index += 2;
            }
            "--note" => {
                notes = Some(
                    tokens
                        .get(index + 1)
                        .context("--note requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown agent-update-release-record option {other}"),
        }
    }
    anyhow::ensure!(
        confirmed,
        "agent-update-release-record requires --confirmed because it records trusted update metadata"
    );
    let artifact_url =
        artifact_url.context("agent-update-release-record requires --artifact-url <https-url>")?;
    let sha256_hex =
        sha256_hex.context("agent-update-release-record requires --sha256-hex <sha256>")?;
    let artifact_signature_hex = artifact_signature_hex
        .context("agent-update-release-record requires --artifact-signature-hex <signature>")?;
    let artifact_signing_key_hex = artifact_signing_key_hex
        .context("agent-update-release-record requires --artifact-signing-key-hex <key>")?;
    validate_update_input(
        &artifact_url,
        &sha256_hex,
        Some(&artifact_signature_hex),
        Some(&artifact_signing_key_hex),
    )?;
    if rollback_artifact_file.is_some() {
        let rollback_artifact_url = rollback_artifact_url
            .as_deref()
            .context("--rollback-artifact-url is required with --rollback-artifact-file")?;
        anyhow::ensure!(
            rollback_artifact_url.starts_with("https://"),
            "rollback artifact URL must use https://"
        );
    } else {
        anyhow::ensure!(
            rollback_artifact_url.is_none() && rollback_signing_seed_hex.is_none(),
            "--rollback-artifact-file is required when rollback URL or rollback signing seed is set"
        );
    }
    Ok(VtyAgentUpdateReleaseRecordRequest {
        name: name.context("agent-update-release-record requires --name <name>")?,
        version: version.context("agent-update-release-record requires --version <version>")?,
        channel,
        artifact_url,
        sha256_hex: sha256_hex.to_ascii_lowercase(),
        artifact_signature_hex: artifact_signature_hex.to_ascii_lowercase(),
        artifact_signing_key_hex: artifact_signing_key_hex.to_ascii_lowercase(),
        rollback_artifact_file,
        rollback_artifact_url,
        rollback_signing_seed_hex,
        size_bytes,
        notes,
    })
}

pub(crate) fn parse_vty_agent_update_artifact_upload(
    tokens: &[&str],
) -> Result<VtyAgentUpdateArtifactUploadRequest> {
    let mut name = None;
    let mut version = None;
    let mut channel = "stable".to_string();
    let mut artifact_file = None;
    let mut signing_seed_hex = None;
    let mut rollback_artifact_file = None;
    let mut rollback_signing_seed_hex = None;
    let mut notes = None;
    let mut stream = false;
    let mut confirmed = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--name" => {
                name = Some(
                    tokens
                        .get(index + 1)
                        .context("--name requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--version" => {
                version = Some(
                    tokens
                        .get(index + 1)
                        .context("--version requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--channel" => {
                channel = tokens
                    .get(index + 1)
                    .context("--channel requires a value")?
                    .to_string();
                index += 2;
            }
            "--artifact-file" => {
                artifact_file = Some(PathBuf::from(
                    tokens
                        .get(index + 1)
                        .context("--artifact-file requires a path")?,
                ));
                index += 2;
            }
            "--signing-seed-hex" => {
                signing_seed_hex = Some(
                    tokens
                        .get(index + 1)
                        .context("--signing-seed-hex requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--rollback-artifact-file" => {
                rollback_artifact_file = Some(PathBuf::from(
                    tokens
                        .get(index + 1)
                        .context("--rollback-artifact-file requires a path")?,
                ));
                index += 2;
            }
            "--rollback-signing-seed-hex" => {
                rollback_signing_seed_hex = Some(
                    tokens
                        .get(index + 1)
                        .context("--rollback-signing-seed-hex requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--note" => {
                notes = Some(
                    tokens
                        .get(index + 1)
                        .context("--note requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--stream" => {
                stream = true;
                index += 1;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown agent-update-artifact-upload option {other}"),
        }
    }
    anyhow::ensure!(
        confirmed,
        "agent-update-artifact-upload requires --confirmed because it uploads trusted update bytes"
    );
    Ok(VtyAgentUpdateArtifactUploadRequest {
        name: name.context("agent-update-artifact-upload requires --name <name>")?,
        version: version.context("agent-update-artifact-upload requires --version <version>")?,
        channel,
        artifact_file: artifact_file
            .context("agent-update-artifact-upload requires --artifact-file <path>")?,
        signing_seed_hex: signing_seed_hex
            .context("agent-update-artifact-upload requires --signing-seed-hex <seed>")?,
        rollback_artifact_file,
        rollback_signing_seed_hex,
        notes,
        stream,
    })
}

pub(crate) fn is_vty_agent_update_releases_command(command: &str) -> bool {
    command == "agent-update-releases"
        || command.starts_with("agent-update-releases ")
        || command == "agent-update-release-latest"
        || command.starts_with("agent-update-release-latest ")
}

pub(crate) fn submit_vty_agent_update_releases(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    if command == "agent-update-release-latest"
        || command.starts_with("agent-update-release-latest ")
    {
        let (name, channel) = parse_vty_latest_release_command(command)?;
        return http_get(
            api_url,
            &format!("/api/v1/agent-update-releases/latest?name={name}&channel={channel}"),
            token,
        );
    }
    let limit = parse_vty_limit_command(command, "agent-update-releases")?;
    http_get(
        api_url,
        &format!("/api/v1/agent-update-releases?limit={limit}"),
        token,
    )
}

pub(crate) fn submit_vty_agent_update_release_record(
    api_url: &str,
    token: Option<&str>,
    request: VtyAgentUpdateReleaseRecordRequest,
) -> Result<String> {
    let rollback_signature =
        if let Some(rollback_artifact_file) = request.rollback_artifact_file.as_ref() {
            let rollback_artifact_url = request
                .rollback_artifact_url
                .as_deref()
                .context("--rollback-artifact-url is required with --rollback-artifact-file")?;
            let rollback_signing_seed_hex = request
                .rollback_signing_seed_hex
                .as_deref()
                .context("--rollback-signing-seed-hex is required with --rollback-artifact-file")?;
            let signature = crate::commands_config::build_update_signature(
                rollback_artifact_file,
                rollback_signing_seed_hex,
            )?;
            validate_update_input(
                rollback_artifact_url,
                &signature.artifact_sha256_hex,
                Some(&signature.artifact_signature_hex),
                Some(&signature.artifact_signing_key_hex),
            )?;
            Some((rollback_artifact_url.to_string(), signature))
        } else {
            None
        };
    http_post_json(
        api_url,
        "/api/v1/agent-update-releases",
        token,
        &serde_json::json!({
            "name": request.name,
            "version": request.version,
            "channel": request.channel,
            "artifact_url": request.artifact_url,
            "artifact_sha256_hex": request.sha256_hex,
            "artifact_signature_hex": request.artifact_signature_hex,
            "artifact_signing_key_hex": request.artifact_signing_key_hex,
            "rollback_artifact_sha256_hex": rollback_signature.as_ref().map(|(_, signature)| signature.artifact_sha256_hex.clone()),
            "rollback_artifact_signature_hex": rollback_signature.as_ref().map(|(_, signature)| signature.artifact_signature_hex.clone()),
            "rollback_artifact_signing_key_hex": rollback_signature.as_ref().map(|(_, signature)| signature.artifact_signing_key_hex.clone()),
            "rollback_artifact_url": rollback_signature.as_ref().map(|(url, _)| url.clone()),
            "rollback_size_bytes": rollback_signature.as_ref().map(|(_, signature)| signature.size_bytes),
            "size_bytes": request.size_bytes,
            "notes": request.notes,
            "confirmed": true,
        }),
    )
}

pub(crate) fn submit_vty_agent_update_artifact_upload(
    api_url: &str,
    token: Option<&str>,
    request: VtyAgentUpdateArtifactUploadRequest,
) -> Result<String> {
    agent_update_artifact_upload_response(
        api_url,
        token,
        request.name,
        request.version,
        request.channel,
        request.artifact_file,
        request.signing_seed_hex,
        request.rollback_artifact_file,
        request.rollback_signing_seed_hex,
        request.notes,
        true,
        request.stream,
    )
}

fn parse_vty_latest_release_command(command: &str) -> Result<(String, String)> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    anyhow::ensure!(
        parts.first() == Some(&"agent-update-release-latest"),
        "usage: agent-update-release-latest [--name <name>] [--channel <channel>]"
    );
    let mut name = "vpsman-agent".to_string();
    let mut channel = "stable".to_string();
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--name" => {
                name = parts
                    .get(index + 1)
                    .context("--name requires a value")?
                    .to_string();
                index += 2;
            }
            "--channel" => {
                channel = parts
                    .get(index + 1)
                    .context("--channel requires a value")?
                    .to_string();
                index += 2;
            }
            other => anyhow::bail!("unknown agent-update-release-latest option {other}"),
        }
    }
    Ok((name, channel))
}

fn parse_vty_limit_command(command: &str, command_name: &'static str) -> Result<u16> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    anyhow::ensure!(
        parts.first() == Some(&command_name),
        "usage: {command_name} [--limit <1-200>]"
    );
    let mut limit = 25_u16;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = parts
                    .get(index + 1)
                    .context("--limit requires a value")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            other => anyhow::bail!("unknown {command_name} option {other}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "{command_name} --limit must be between 1 and 200"
    );
    Ok(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use vpsman_common::sign_update_artifact_hash;

    #[test]
    fn parses_vty_agent_update_release_record() {
        let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
        let sha256_hex = "aa".repeat(32);
        let request = parse_vty_agent_update_release_record(&[
            "--name",
            "vpsman-agent",
            "--version",
            "1.2.3",
            "--channel",
            "stable",
            "--artifact-url",
            "https://updates.example/vpsman-agent",
            "--sha256-hex",
            &sha256_hex,
            "--artifact-signature-hex",
            &hex::encode(sign_update_artifact_hash(&signing_key, &sha256_hex)),
            "--artifact-signing-key-hex",
            &hex::encode(signing_key.verifying_key().to_bytes()),
            "--rollback-artifact-file",
            "./target/vpsman-agent.rollback",
            "--rollback-artifact-url",
            "https://updates.example/vpsman-agent.rollback",
            "--rollback-signing-seed-hex",
            &"10".repeat(32),
            "--size-bytes",
            "1024",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.name, "vpsman-agent");
        assert_eq!(request.version, "1.2.3");
        assert_eq!(request.channel, "stable");
        assert_eq!(request.size_bytes, Some(1024));
        assert_eq!(
            request.rollback_artifact_file.as_deref(),
            Some(std::path::Path::new("./target/vpsman-agent.rollback"))
        );
        assert_eq!(
            request.rollback_artifact_url.as_deref(),
            Some("https://updates.example/vpsman-agent.rollback")
        );
    }

    #[test]
    fn parses_vty_agent_update_artifact_upload() {
        let rollback_seed = "22".repeat(32);
        let request = parse_vty_agent_update_artifact_upload(&[
            "--name",
            "vpsman-agent",
            "--version",
            "1.2.3",
            "--channel",
            "stable",
            "--artifact-file",
            "./target/vpsman-agent",
            "--signing-seed-hex",
            &"11".repeat(32),
            "--rollback-artifact-file",
            "./target/vpsman-agent.rollback",
            "--rollback-signing-seed-hex",
            &rollback_seed,
            "--note",
            "hosted",
            "--stream",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.name, "vpsman-agent");
        assert_eq!(request.version, "1.2.3");
        assert_eq!(request.channel, "stable");
        assert!(request.rollback_artifact_file.is_some());
        assert_eq!(
            request.rollback_signing_seed_hex.as_deref(),
            Some(rollback_seed.as_str())
        );
        assert_eq!(request.notes.as_deref(), Some("hosted"));
        assert!(request.stream);
    }

    #[test]
    fn parses_vty_latest_release_command() {
        let (name, channel) = parse_vty_latest_release_command(
            "agent-update-release-latest --name vpsman-agent --channel beta",
        )
        .unwrap();
        assert_eq!(name, "vpsman-agent");
        assert_eq!(channel, "beta");
    }

    #[test]
    fn rejects_unconfirmed_or_bad_vty_agent_update_release_record() {
        assert!(parse_vty_agent_update_release_record(&["--name", "vpsman-agent"]).is_err());
        assert!(parse_vty_agent_update_artifact_upload(&[
            "--name",
            "vpsman-agent",
            "--artifact-file",
            "./agent.bin"
        ])
        .is_err());
        assert!(submit_vty_agent_update_releases(
            "http://127.0.0.1:1",
            None,
            "agent-update-releases --limit 0"
        )
        .is_err());
    }
}
