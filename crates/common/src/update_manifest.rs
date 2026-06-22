use serde::Deserialize;

const VERSION_MANIFEST_SCHEMA_VERSION: u16 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentUpdateManifestCandidate {
    pub asset_name: String,
    pub artifact_url: String,
    pub checksum_url: String,
    pub sha256_hex: String,
}

#[derive(Debug, Deserialize)]
struct VersionManifestHeader {
    schema_version: u16,
}

#[derive(Debug, Deserialize)]
struct VersionManifest {
    project: String,
    #[serde(default)]
    assets: Vec<VersionManifestAsset>,
    #[serde(default)]
    checksum_manifest: Option<VersionManifestDownload>,
}

#[derive(Debug, Deserialize)]
struct VersionManifestAsset {
    name: String,
    download_url: String,
}

#[derive(Debug, Deserialize)]
struct VersionManifestDownload {
    name: String,
    download_url: String,
}

pub fn agent_update_asset_name_for_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("vpsman-agent-linux-x86_64-musl"),
        "aarch64" => Some("vpsman-agent-linux-aarch64-musl"),
        _ => None,
    }
}

pub fn resolve_agent_update_manifest_candidate(
    manifest_bytes: &[u8],
    checksum_bytes: &[u8],
    arch: &str,
) -> Result<AgentUpdateManifestCandidate, String> {
    let header: VersionManifestHeader = serde_json::from_slice(manifest_bytes)
        .map_err(|error| format!("failed to parse update manifest header: {error}"))?;
    if header.schema_version != VERSION_MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "unsupported update manifest schema {}",
            header.schema_version
        ));
    }
    let manifest: VersionManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| format!("failed to parse update manifest: {error}"))?;
    if manifest.project != "vpsman" {
        return Err(format!(
            "unexpected update manifest project {}",
            manifest.project
        ));
    }
    let asset_name = agent_update_asset_name_for_arch(arch)
        .ok_or_else(|| format!("unsupported agent update architecture {arch}"))?;
    let asset = manifest
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| format!("update manifest missing asset {asset_name}"))?;
    let checksum_manifest = manifest
        .checksum_manifest
        .as_ref()
        .ok_or_else(|| "update manifest checksum entry is missing".to_string())?;
    if checksum_manifest.name != "SHA256SUMS" {
        return Err("update manifest checksum entry must be SHA256SUMS".to_string());
    }
    let checksums = std::str::from_utf8(checksum_bytes)
        .map_err(|error| format!("update checksum manifest is not UTF-8: {error}"))?;
    let sha256_hex = checksum_for_asset(checksums, asset_name)?;
    Ok(AgentUpdateManifestCandidate {
        asset_name: asset_name.to_string(),
        artifact_url: manifest_download_url(&asset.download_url, asset_name)?,
        checksum_url: manifest_download_url(&checksum_manifest.download_url, "SHA256SUMS")?,
        sha256_hex,
    })
}

pub fn update_manifest_checksum_url(manifest_bytes: &[u8]) -> Result<String, String> {
    let header: VersionManifestHeader = serde_json::from_slice(manifest_bytes)
        .map_err(|error| format!("failed to parse update manifest header: {error}"))?;
    if header.schema_version != VERSION_MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "unsupported update manifest schema {}",
            header.schema_version
        ));
    }
    let manifest: VersionManifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| format!("failed to parse update manifest: {error}"))?;
    if manifest.project != "vpsman" {
        return Err(format!(
            "unexpected update manifest project {}",
            manifest.project
        ));
    }
    let checksum_manifest = manifest
        .checksum_manifest
        .as_ref()
        .ok_or_else(|| "update manifest checksum entry is missing".to_string())?;
    if checksum_manifest.name != "SHA256SUMS" {
        return Err("update manifest checksum entry must be SHA256SUMS".to_string());
    }
    manifest_download_url(&checksum_manifest.download_url, "SHA256SUMS")
}

fn manifest_download_url(value: &str, asset_name: &str) -> Result<String, String> {
    if asset_name.contains('/') || asset_name.contains('\\') || asset_name.is_empty() {
        return Err("update manifest asset name is invalid".to_string());
    }
    let value = value.trim();
    if value.is_empty() {
        return Err(format!(
            "update manifest download URL for {asset_name} is empty"
        ));
    }
    if value.contains('#') {
        return Err(format!(
            "update manifest download URL for {asset_name} must not contain a fragment"
        ));
    }
    if !(value.starts_with("https://")
        || value.starts_with("file://")
        || value.starts_with("http://localhost")
        || value.starts_with("http://127.0.0.1")
        || value.starts_with("http://[::1]"))
    {
        return Err(format!(
            "update manifest download URL for {asset_name} must use https://"
        ));
    }
    Ok(value.to_string())
}

fn checksum_for_asset(checksums: &str, asset_name: &str) -> Result<String, String> {
    for line in checksums.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        let name = name.trim_start_matches('*');
        let basename = name.rsplit('/').next().unwrap_or(name);
        if name == asset_name || basename == asset_name {
            return normalize_sha256(hash);
        }
    }
    Err(format!(
        "update checksum manifest does not contain {asset_name}"
    ))
}

fn normalize_sha256(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() != 64 || !value.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return Err("sha256_hex must be 64 lowercase or uppercase hex characters".to_string());
    }
    Ok(value)
}
