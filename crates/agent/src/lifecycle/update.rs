use std::{env, fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use anyhow::{anyhow, Context, Result};
use reqwest::{redirect, Url};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    sync::{mpsc, oneshot},
    time,
};
use vpsman_common::{
    AgentUpdateVerificationRequest, AgentUpdateVerificationResult, CommandOutput, OutputStream,
};

use crate::{
    agent_binary_path::{current_agent_binary_path, rollback_path, staged_path},
    command_worker::CommandCancelToken,
    update_activation::{execute_update_activate, AgentUpdateActivateInput},
};

const MAX_UPDATE_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20);
const UPDATE_ROOT_CERT_PEM_ENV: &str = "VPSMAN_UPDATE_ROOT_CERT_PEM";
const VERSION_MANIFEST_SCHEMA_VERSION: u16 = 3;

#[derive(Clone)]
pub(crate) struct AgentUpdateInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) artifact_url: &'a str,
    pub(crate) sha256_hex: &'a str,
    pub(crate) max_timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

#[derive(Clone)]
pub(crate) struct AgentUpdateCheckInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) version_url: &'a str,
    pub(crate) activate: bool,
    pub(crate) restart_agent: bool,
    pub(crate) max_timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
    pub(crate) verification_tx: Option<AgentUpdateVerificationSender>,
}

pub(crate) type AgentUpdateVerificationSender = mpsc::Sender<AgentUpdateVerificationWork>;

pub(crate) struct AgentUpdateVerificationWork {
    pub(crate) request: AgentUpdateVerificationRequest,
    pub(crate) response: oneshot::Sender<AgentUpdateVerificationResult>,
}

pub(crate) async fn execute_update_agent(
    input: AgentUpdateInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let current_exe = current_agent_binary_path()?;
    let timeout = Duration::from_secs(input.max_timeout_secs.max(1));
    input.cancel_token.check("agent_update")?;
    let artifact_url = input.artifact_url.to_string();
    let sha256_hex = normalize_sha256(input.sha256_hex)?;
    let job_id = input.job_id;
    let output = match time::timeout(
        timeout,
        stage_update_artifact(UpdateStageInput {
            job_id,
            artifact_url: &artifact_url,
            expected_sha256_hex: &sha256_hex,
            current_exe: &current_exe,
            cancel_token: &input.cancel_token,
        }),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            input
                .cancel_token
                .cancel(format!("timeout after {}s", input.max_timeout_secs.max(1)));
            return Err(anyhow!("agent update staging timed out"));
        }
    };
    Ok(vec![output])
}

pub(crate) async fn execute_update_check(
    input: AgentUpdateCheckInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let current_exe = current_agent_binary_path()?;
    let timeout = Duration::from_secs(input.max_timeout_secs.max(1));
    input.cancel_token.check("agent_update_check")?;
    let job_id = input.job_id;
    let version_url = input.version_url.trim().to_string();
    let check = match time::timeout(
        timeout,
        check_and_stage_update(CheckStageInput {
            job_id,
            version_url: &version_url,
            current_exe: &current_exe,
            cancel_token: &input.cancel_token,
            verification_tx: input.verification_tx.clone(),
        }),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            input
                .cancel_token
                .cancel(format!("timeout after {}s", input.max_timeout_secs.max(1)));
            return Err(anyhow!("agent update check timed out"));
        }
    };

    let mut outputs = check.outputs;
    if let Some(staged_sha256_hex) = check.staged_sha256_hex {
        if input.activate {
            if let Some(last) = outputs.last_mut() {
                last.done = false;
                last.exit_code = None;
            }
            outputs.extend(
                execute_update_activate(AgentUpdateActivateInput {
                    job_id: input.job_id,
                    staged_sha256_hex,
                    restart_agent: input.restart_agent,
                    max_timeout_secs: input.max_timeout_secs,
                    cancel_token: input.cancel_token.clone(),
                })
                .await?,
            );
        }
    }
    Ok(outputs)
}

struct UpdateStageInput<'a> {
    job_id: uuid::Uuid,
    artifact_url: &'a str,
    expected_sha256_hex: &'a str,
    current_exe: &'a Path,
    cancel_token: &'a CommandCancelToken,
}

struct CheckStageInput<'a> {
    job_id: uuid::Uuid,
    version_url: &'a str,
    current_exe: &'a Path,
    cancel_token: &'a CommandCancelToken,
    verification_tx: Option<AgentUpdateVerificationSender>,
}

struct CheckStageResult {
    outputs: Vec<CommandOutput>,
    staged_sha256_hex: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateVersionStatus {
    Current,
    Newer,
    DowngradeBlocked,
    NotOrderable,
}

#[derive(Debug, Deserialize)]
struct VersionManifestHeader {
    schema_version: u16,
}

#[derive(Debug, Deserialize)]
struct VersionManifest {
    project: String,
    version: String,
    tag: String,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    assets: Vec<VersionManifestAsset>,
}

#[derive(Debug, Deserialize)]
struct VersionManifestAsset {
    name: String,
    download_url: String,
}

async fn stage_update_artifact(input: UpdateStageInput<'_>) -> Result<CommandOutput> {
    input.cancel_token.check("agent_update")?;
    let artifact = fetch_update_artifact(input.artifact_url).await?;
    input.cancel_token.check("agent_update")?;
    let observed_sha256_hex = sha256_hex(&artifact);
    if observed_sha256_hex != input.expected_sha256_hex {
        anyhow::bail!(
            "agent update artifact hash mismatch: expected {}, got {observed_sha256_hex}",
            input.expected_sha256_hex
        );
    }
    stage_downloaded_update_artifact(
        input.job_id,
        input.current_exe,
        input.cancel_token,
        &artifact,
        &observed_sha256_hex,
    )
}

fn stage_downloaded_update_artifact(
    job_id: uuid::Uuid,
    current_exe: &Path,
    cancel_token: &CommandCancelToken,
    artifact: &[u8],
    observed_sha256_hex: &str,
) -> Result<CommandOutput> {
    let staged_path = staged_path(current_exe)?;
    let rollback_path = rollback_path(current_exe)?;
    cancel_token.check("agent_update")?;
    persist_staged_artifact(current_exe, &staged_path, &rollback_path, artifact)?;
    let status = serde_json::json!({
        "type": "agent_update",
        "status": "staged",
        "sha256_hex": observed_sha256_hex,
        "size_bytes": artifact.len(),
        "staged_path": staged_path.display().to_string(),
        "rollback_path": rollback_path.display().to_string(),
        "activation": "manual_restart_required",
    });
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    })
}

async fn check_and_stage_update(input: CheckStageInput<'_>) -> Result<CheckStageResult> {
    input.cancel_token.check("agent_update_check")?;
    let manifest_bytes = fetch_update_artifact(input.version_url)
        .await
        .with_context(|| format!("failed to fetch update manifest {}", input.version_url))?;
    input.cancel_token.check("agent_update_check")?;
    let header: VersionManifestHeader = serde_json::from_slice(&manifest_bytes)
        .context("failed to parse update manifest header")?;
    if !matches!(header.schema_version, 2 | VERSION_MANIFEST_SCHEMA_VERSION) {
        anyhow::bail!(
            "unsupported update manifest schema {}",
            header.schema_version
        );
    }
    let manifest: VersionManifest =
        serde_json::from_slice(&manifest_bytes).context("failed to parse update manifest")?;
    if manifest.project != "vpsman" {
        anyhow::bail!("unexpected update manifest project {}", manifest.project);
    }
    let current_version = crate::build_info::agent_release_version();
    let Some(asset_name) = agent_asset_name() else {
        let status = serde_json::json!({
            "type": "agent_update_check",
            "status": "unsupported_arch",
            "arch": std::env::consts::ARCH,
            "version_url": input.version_url,
            "candidate_version": manifest.version,
            "tag": manifest.tag,
            "commit": manifest.commit,
        });
        return Ok(CheckStageResult {
            outputs: vec![status_output(input.job_id, status, Some(2), true)?],
            staged_sha256_hex: None,
        });
    };
    match candidate_version_status(current_version, &manifest.version) {
        CandidateVersionStatus::Current => {
            let status = serde_json::json!({
                "type": "agent_update_check",
                "status": "current",
                "current_version": current_version,
                "candidate_version": manifest.version,
                "tag": manifest.tag,
                "commit": manifest.commit,
                "asset": asset_name,
                "version_url": input.version_url,
            });
            return Ok(CheckStageResult {
                outputs: vec![status_output(input.job_id, status, Some(0), true)?],
                staged_sha256_hex: None,
            });
        }
        CandidateVersionStatus::DowngradeBlocked => {
            let status = serde_json::json!({
                "type": "agent_update_check",
                "status": "downgrade_blocked",
                "current_version": current_version,
                "candidate_version": manifest.version,
                "tag": manifest.tag,
                "commit": manifest.commit,
                "asset": asset_name,
                "version_url": input.version_url,
            });
            return Ok(CheckStageResult {
                outputs: vec![status_output(input.job_id, status, Some(2), true)?],
                staged_sha256_hex: None,
            });
        }
        CandidateVersionStatus::NotOrderable => {
            let status = serde_json::json!({
                "type": "agent_update_check",
                "status": "version_not_orderable",
                "current_version": current_version,
                "candidate_version": manifest.version,
                "tag": manifest.tag,
                "commit": manifest.commit,
                "asset": asset_name,
                "version_url": input.version_url,
            });
            return Ok(CheckStageResult {
                outputs: vec![status_output(input.job_id, status, Some(2), true)?],
                staged_sha256_hex: None,
            });
        }
        CandidateVersionStatus::Newer => {}
    }
    let Some(asset) = manifest
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
    else {
        let status = serde_json::json!({
            "type": "agent_update_check",
            "status": "asset_missing",
            "arch": std::env::consts::ARCH,
            "asset": asset_name,
            "candidate_version": manifest.version,
            "tag": manifest.tag,
            "commit": manifest.commit,
            "version_url": input.version_url,
        });
        return Ok(CheckStageResult {
            outputs: vec![status_output(input.job_id, status, Some(2), true)?],
            staged_sha256_hex: None,
        });
    };

    let artifact_url = manifest_download_url(&asset.download_url, asset_name)?;
    let artifact = fetch_update_artifact(&artifact_url)
        .await
        .with_context(|| format!("failed to fetch update artifact {artifact_url}"))?;
    input.cancel_token.check("agent_update_check")?;
    let observed_sha256_hex = sha256_hex(&artifact);
    verify_agent_update_candidate(
        &input,
        AgentUpdateVerificationRequest {
            job_id: input.job_id,
            version_url: input.version_url.to_string(),
            artifact_url: artifact_url.clone(),
            asset_name: asset_name.to_string(),
            sha256_hex: observed_sha256_hex.clone(),
        },
    )
    .await?;
    let check_status = serde_json::json!({
        "type": "agent_update_check",
        "status": "staging",
        "current_version": current_version,
        "candidate_version": manifest.version,
        "tag": manifest.tag,
        "commit": manifest.commit,
        "asset": asset_name,
        "artifact_url": artifact_url,
        "sha256_hex": observed_sha256_hex,
        "version_url": input.version_url,
    });
    let mut outputs = vec![status_output(input.job_id, check_status, None, false)?];
    let staged = stage_downloaded_update_artifact(
        input.job_id,
        input.current_exe,
        input.cancel_token,
        &artifact,
        &observed_sha256_hex,
    )?;
    outputs.push(staged);
    Ok(CheckStageResult {
        outputs,
        staged_sha256_hex: Some(observed_sha256_hex),
    })
}

async fn verify_agent_update_candidate(
    input: &CheckStageInput<'_>,
    request: AgentUpdateVerificationRequest,
) -> Result<()> {
    let Some(verification_tx) = input.verification_tx.as_ref() else {
        return Ok(());
    };
    let job_id = request.job_id;
    let (response_tx, response_rx) = oneshot::channel();
    verification_tx
        .send(AgentUpdateVerificationWork {
            request,
            response: response_tx,
        })
        .await
        .context("agent update verification request failed")?;
    input.cancel_token.check("agent_update_check")?;
    let result = response_rx
        .await
        .context("agent update verification response dropped")?;
    input.cancel_token.check("agent_update_check")?;
    if result.job_id != job_id {
        anyhow::bail!("agent update verification response identity mismatch");
    }
    if !result.approved {
        anyhow::bail!("agent update verification rejected: {}", result.message);
    }
    Ok(())
}

fn candidate_version_status(
    current_version: &str,
    candidate_version: &str,
) -> CandidateVersionStatus {
    let Ok(current) = Version::parse(current_version.trim()) else {
        return CandidateVersionStatus::NotOrderable;
    };
    let Ok(candidate) = Version::parse(candidate_version.trim()) else {
        return CandidateVersionStatus::NotOrderable;
    };
    if candidate == current {
        CandidateVersionStatus::Current
    } else if candidate > current {
        CandidateVersionStatus::Newer
    } else {
        CandidateVersionStatus::DowngradeBlocked
    }
}

fn persist_staged_artifact(
    current_exe: &Path,
    staged_path: &Path,
    rollback_path: &Path,
    artifact: &[u8],
) -> Result<()> {
    if current_exe.exists() {
        fs::copy(current_exe, rollback_path).with_context(|| {
            format!(
                "failed to write agent update rollback copy {}",
                rollback_path.display()
            )
        })?;
    }
    let temp_path = staged_path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    if let Err(error) = fs::write(&temp_path, artifact) {
        let _ = fs::remove_file(&temp_path);
        return Err(error)
            .with_context(|| format!("failed to write staged update {}", temp_path.display()));
    }
    if let Err(error) = fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755)) {
        let _ = fs::remove_file(&temp_path);
        return Err(error)
            .with_context(|| format!("failed to set executable mode on {}", temp_path.display()));
    }
    if let Err(error) = fs::rename(&temp_path, staged_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error).with_context(|| {
            format!(
                "failed to atomically move staged update {} to {}",
                temp_path.display(),
                staged_path.display()
            )
        });
    }
    Ok(())
}

fn status_output(
    job_id: uuid::Uuid,
    status: serde_json::Value,
    exit_code: Option<i32>,
    done: bool,
) -> Result<CommandOutput> {
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code,
        done,
    })
}

fn agent_asset_name() -> Option<&'static str> {
    vpsman_common::agent_update_asset_name_for_arch(std::env::consts::ARCH)
}

fn manifest_download_url(value: &str, asset_name: &str) -> Result<String> {
    if asset_name.contains('/') || asset_name.contains('\\') || asset_name.is_empty() {
        anyhow::bail!("update manifest asset name is invalid");
    }
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("update manifest download URL for {asset_name} is empty");
    }
    parse_artifact_url(value)
        .with_context(|| format!("update manifest download URL for {asset_name} is invalid"))?;
    Ok(value.to_string())
}

async fn fetch_update_artifact(artifact_url: &str) -> Result<Vec<u8>> {
    let parsed = parse_artifact_url(artifact_url)?;
    match parsed.scheme {
        ArtifactScheme::File => read_file_artifact(&parsed),
        ArtifactScheme::Https | ArtifactScheme::HttpLocalDev => fetch_http_artifact(&parsed).await,
    }
}

fn read_file_artifact(parsed: &ParsedArtifactUrl) -> Result<Vec<u8>> {
    let path = parsed
        .path_and_query
        .strip_prefix('/')
        .map(|_| Path::new(&parsed.path_and_query))
        .context("file update artifact URL requires an absolute path")?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to stat update artifact {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("update artifact is not a regular file");
    }
    if metadata.len() > MAX_UPDATE_ARTIFACT_BYTES as u64 {
        anyhow::bail!("update artifact exceeds {MAX_UPDATE_ARTIFACT_BYTES} bytes");
    }
    fs::read(path).with_context(|| format!("failed to read update artifact {}", path.display()))
}

async fn fetch_http_artifact(parsed: &ParsedArtifactUrl) -> Result<Vec<u8>> {
    let client = update_http_client()?;
    let url = Url::parse(&parsed.http_url()).context("update artifact URL is invalid")?;
    if !update_http_url_allowed(&url) {
        anyhow::bail!("update artifact URL is not allowed");
    }
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .send()
        .await
        .context("failed to fetch update artifact")?
        .error_for_status()
        .context("update artifact endpoint returned an error")?;
    read_limited_response(response).await
}

fn update_http_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .connect_timeout(UPDATE_DOWNLOAD_TIMEOUT)
        .timeout(UPDATE_DOWNLOAD_TIMEOUT)
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("update artifact redirect limit exceeded");
            }
            if update_redirect_url_allowed(attempt.previous().last(), attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("update artifact redirect target is not allowed")
            }
        }))
        .user_agent(format!(
            "vpsman-agent/{} build/{}",
            crate::build_info::agent_release_version(),
            crate::build_info::AGENT_BUILD_NUMBER
        ));
    let Ok(path) = env::var(UPDATE_ROOT_CERT_PEM_ENV) else {
        return builder
            .build()
            .context("failed to build update HTTP client");
    };
    if path.trim().is_empty() {
        return builder
            .build()
            .context("failed to build update HTTP client");
    }
    let pem = fs::read(&path)
        .with_context(|| format!("failed to read update root certificate PEM from {path}"))?;
    let certificates = reqwest::Certificate::from_pem_bundle(&pem)
        .context("failed to parse update root certificate PEM")?;
    if certificates.is_empty() {
        anyhow::bail!("update root certificate PEM contained no certificates");
    }
    for certificate in certificates {
        builder = builder.add_root_certificate(certificate);
    }
    builder
        .build()
        .context("failed to build update HTTP client")
}

async fn read_limited_response(mut response: reqwest::Response) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_UPDATE_ARTIFACT_BYTES as u64)
    {
        anyhow::bail!("update artifact exceeds {MAX_UPDATE_ARTIFACT_BYTES} bytes");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("failed to read update artifact body")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_UPDATE_ARTIFACT_BYTES {
            anyhow::bail!("update artifact exceeds {MAX_UPDATE_ARTIFACT_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn normalize_sha256(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() != 64 || !value.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        anyhow::bail!("sha256_hex must be 64 lowercase or uppercase hex characters");
    }
    Ok(value)
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArtifactScheme {
    File,
    Https,
    HttpLocalDev,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedArtifactUrl {
    scheme: ArtifactScheme,
    host: String,
    port: u16,
    path_and_query: String,
}

impl ParsedArtifactUrl {
    fn http_url(&self) -> String {
        let scheme = match self.scheme {
            ArtifactScheme::Https => "https",
            ArtifactScheme::HttpLocalDev => "http",
            ArtifactScheme::File => unreachable!("file artifacts are not fetched over HTTP"),
        };
        format!(
            "{scheme}://{}{}",
            self.authority_for_url(),
            self.path_and_query
        )
    }

    fn authority_for_url(&self) -> String {
        let default_port = match self.scheme {
            ArtifactScheme::File => 0,
            ArtifactScheme::Https => 443,
            ArtifactScheme::HttpLocalDev => 80,
        };
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == default_port {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

fn parse_artifact_url(url: &str) -> Result<ParsedArtifactUrl> {
    let trimmed = url.trim();
    if trimmed.contains('#') {
        anyhow::bail!("update artifact URL fragments are not supported");
    }
    if let Some(path) = trimmed.strip_prefix("file://") {
        if !path.starts_with('/') {
            anyhow::bail!("file update artifact URL requires an absolute path");
        }
        return Ok(ParsedArtifactUrl {
            scheme: ArtifactScheme::File,
            host: String::new(),
            port: 0,
            path_and_query: path.to_string(),
        });
    }
    let (scheme, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
        (ArtifactScheme::Https, rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        (ArtifactScheme::HttpLocalDev, rest)
    } else {
        anyhow::bail!("update artifact URL must use https://");
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let suffix = &rest[authority_end..];
    if authority.is_empty() || authority.contains('@') {
        anyhow::bail!("update artifact URL authority is invalid");
    }
    let (host, port) = parse_authority(authority, scheme)?;
    if scheme == ArtifactScheme::HttpLocalDev && !is_localhost(&host) {
        anyhow::bail!("http:// update artifacts are allowed only for localhost development");
    }
    let path_and_query = if suffix.is_empty() {
        "/".to_string()
    } else if suffix.starts_with('?') {
        format!("/{suffix}")
    } else {
        suffix.to_string()
    };
    Ok(ParsedArtifactUrl {
        scheme,
        host,
        port,
        path_and_query,
    })
}

fn parse_authority(authority: &str, scheme: ArtifactScheme) -> Result<(String, u16)> {
    let default_port = match scheme {
        ArtifactScheme::File => 0,
        ArtifactScheme::Https => 443,
        ArtifactScheme::HttpLocalDev => 80,
    };
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .context("invalid bracketed IPv6 update artifact URL host")?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix
                .strip_prefix(':')
                .context("invalid bracketed IPv6 update artifact URL port")?
                .parse::<u16>()
                .context("invalid update artifact URL port")?
        };
        return Ok((host.to_string(), port));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (
            host,
            port.parse::<u16>()
                .context("invalid update artifact URL port")?,
        ),
        _ => (authority, default_port),
    };
    if host.trim().is_empty() {
        anyhow::bail!("update artifact URL host is empty");
    }
    Ok((host.to_string(), port))
}

fn is_localhost(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn update_http_url_allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => url.host_str().is_some_and(is_localhost),
        _ => false,
    }
}

fn update_redirect_url_allowed(previous: Option<&Url>, next: &Url) -> bool {
    match next.scheme() {
        "https" => true,
        "http" => {
            next.host_str().is_some_and(is_localhost)
                && previous.is_some_and(|url| {
                    url.scheme() == "http" && url.host_str().is_some_and(is_localhost)
                })
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "tests_update.rs"]
mod tests;
