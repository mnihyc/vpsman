use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    http::{http_get, http_get_to_file, http_post_json},
    util::percent_encode_query_value,
};

pub(crate) fn file_transfers(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    client_id: Option<String>,
    session_id: Option<String>,
) -> Result<()> {
    println!(
        "{}",
        file_transfers_output(api_url, token, limit, client_id, session_id)?
    );
    Ok(())
}

pub(crate) fn file_transfers_output(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    client_id: Option<String>,
    session_id: Option<String>,
) -> Result<String> {
    let path = file_transfers_path(limit, client_id.as_deref(), session_id.as_deref())?;
    http_get(api_url, &path, token)
}

pub(crate) fn file_transfer_sources(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!("{}", file_transfer_sources_output(api_url, token, limit)?);
    Ok(())
}

pub(crate) fn file_transfer_sources_output(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
) -> Result<String> {
    http_get(api_url, &file_transfer_sources_path(limit), token)
}

pub(crate) fn file_transfer_sources_path(limit: u16) -> String {
    format!(
        "/api/v1/file-transfer-sources?limit={}",
        limit.clamp(1, 200)
    )
}

pub(crate) fn file_transfers_path(
    limit: u16,
    client_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<String> {
    let mut path = format!("/api/v1/file-transfers?limit={}", limit.clamp(1, 200));
    if let Some(client_id) = client_id.map(str::trim).filter(|value| !value.is_empty()) {
        anyhow::ensure!(
            client_id.len() <= 128,
            "--client-id must be at most 128 bytes"
        );
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(client_id));
    }
    if let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) {
        let session_id = Uuid::parse_str(session_id).context("invalid --session-id UUID")?;
        path.push_str("&session_id=");
        path.push_str(&session_id.to_string());
    }
    Ok(path)
}

pub(crate) fn file_transfer_handoff(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    session_id: String,
    output_file: Option<PathBuf>,
    confirmed: bool,
) -> Result<()> {
    println!(
        "{}",
        file_transfer_handoff_output(
            api_url,
            token,
            client_id,
            session_id,
            output_file,
            confirmed,
        )?
    );
    Ok(())
}

pub(crate) fn file_transfer_handoff_output(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    session_id: String,
    output_file: Option<PathBuf>,
    confirmed: bool,
) -> Result<String> {
    anyhow::ensure!(confirmed, "file-transfer-handoff requires --confirmed");
    let session_id = Uuid::parse_str(&session_id).context("invalid --session-id UUID")?;
    let path = file_transfer_handoff_path(&client_id, session_id);
    let response = http_post_json(
        api_url,
        &path,
        token,
        &serde_json::json!({ "confirmed": confirmed }),
    )?;
    if let Some(output_file) = output_file {
        let downloaded_size_bytes =
            http_get_to_file(api_url, &format!("{path}/artifact"), token, &output_file)?;
        let mut value: serde_json::Value = serde_json::from_str(&response)?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "output".to_string(),
                serde_json::json!(output_file.to_string_lossy().to_string()),
            );
            object.insert(
                "downloaded_size_bytes".to_string(),
                serde_json::json!(downloaded_size_bytes),
            );
        }
        Ok(value.to_string())
    } else {
        Ok(response)
    }
}

pub(crate) fn file_transfer_source_upload(
    api_url: &str,
    token: Option<&str>,
    source: PathBuf,
    name: Option<String>,
    confirmed: bool,
) -> Result<()> {
    println!(
        "{}",
        file_transfer_source_upload_output(api_url, token, source, name, confirmed)?
    );
    Ok(())
}

pub(crate) fn file_transfer_source_upload_output(
    api_url: &str,
    token: Option<&str>,
    source: PathBuf,
    name: Option<String>,
    confirmed: bool,
) -> Result<String> {
    anyhow::ensure!(
        confirmed,
        "file-transfer-source-upload requires --confirmed"
    );
    let bytes =
        std::fs::read(&source).with_context(|| format!("failed to read {}", source.display()))?;
    let sha256_hex = hex::encode(Sha256::digest(&bytes));
    let fallback_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToString::to_string);
    http_post_json(
        api_url,
        "/api/v1/file-transfer-sources",
        token,
        &serde_json::json!({
            "name": name.or(fallback_name),
            "source_base64": BASE64.encode(&bytes),
            "sha256_hex": sha256_hex,
            "size_bytes": bytes.len(),
            "confirmed": confirmed,
        }),
    )
}

pub(crate) fn file_transfer_source_download(
    api_url: &str,
    token: Option<&str>,
    artifact_id: String,
    output_file: PathBuf,
) -> Result<()> {
    println!(
        "{}",
        file_transfer_source_download_output(api_url, token, artifact_id, output_file)?
    );
    Ok(())
}

pub(crate) fn file_transfer_source_download_output(
    api_url: &str,
    token: Option<&str>,
    artifact_id: String,
    output_file: PathBuf,
) -> Result<String> {
    let artifact_id = Uuid::parse_str(&artifact_id).context("invalid --artifact-id UUID")?;
    let path = file_transfer_source_download_path(artifact_id);
    let downloaded_size_bytes = http_get_to_file(api_url, &path, token, &output_file)?;
    Ok(serde_json::json!({
        "artifact_id": artifact_id,
        "output": output_file.to_string_lossy().to_string(),
        "downloaded_size_bytes": downloaded_size_bytes,
    })
    .to_string())
}

fn file_transfer_handoff_path(client_id: &str, session_id: Uuid) -> String {
    format!(
        "/api/v1/file-transfers/{}/{session_id}/handoff",
        percent_encode_path_segment(client_id)
    )
}

pub(crate) fn file_transfer_source_download_path(artifact_id: Uuid) -> String {
    format!("/api/v1/file-transfer-sources/{artifact_id}/artifact")
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        file_transfer_handoff_path, file_transfer_source_download_path, file_transfer_sources_path,
        file_transfers_path,
    };
    use uuid::Uuid;

    #[test]
    fn builds_filtered_file_transfers_path() {
        let path = file_transfers_path(
            500,
            Some("edge a"),
            Some("11111111-2222-4333-8444-555555555555"),
        )
        .unwrap();

        assert_eq!(
            path,
            "/api/v1/file-transfers?limit=200&client_id=edge%20a&session_id=11111111-2222-4333-8444-555555555555"
        );
    }

    #[test]
    fn builds_file_transfer_handoff_path() {
        let session_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
        assert_eq!(
            file_transfer_handoff_path("edge a", session_id),
            "/api/v1/file-transfers/edge%20a/11111111-2222-4333-8444-555555555555/handoff"
        );
    }

    #[test]
    fn builds_file_transfer_source_paths() {
        let artifact_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
        assert_eq!(
            file_transfer_sources_path(500),
            "/api/v1/file-transfer-sources?limit=200"
        );
        assert_eq!(
            file_transfer_source_download_path(artifact_id),
            "/api/v1/file-transfer-sources/11111111-2222-4333-8444-555555555555/artifact"
        );
    }
}
