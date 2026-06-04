use std::{
    env, fs,
    io::{Read, Write},
    net::{TcpStream as StdTcpStream, ToSocketAddrs},
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use rustls_pki_types::{CertificateDer, ServerName};
use sha2::{Digest, Sha256};
use tokio::{task, time};
use vpsman_common::{verify_update_artifact_signature, CommandOutput, OutputStream};

use crate::agent_binary_path::{current_agent_binary_path, rollback_path, staged_path};

const MAX_UPDATE_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20);
const UPDATE_ROOT_CERT_PEM_ENV: &str = "VPSMAN_UPDATE_ROOT_CERT_PEM";

#[derive(Clone, Copy)]
pub(crate) struct AgentUpdateInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) artifact_url: &'a str,
    pub(crate) sha256_hex: &'a str,
    pub(crate) artifact_signature_hex: Option<&'a str>,
    pub(crate) artifact_signing_key_hex: Option<&'a str>,
    pub(crate) trusted_artifact_signing_key_hex: Option<&'a str>,
    pub(crate) timeout_secs: u64,
}

pub(crate) async fn execute_update_agent(
    input: AgentUpdateInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let current_exe = current_agent_binary_path()?;
    let timeout = Duration::from_secs(input.timeout_secs.max(1));
    let artifact_url = input.artifact_url.to_string();
    let sha256_hex = normalize_sha256(input.sha256_hex)?;
    let artifact_signature_hex =
        normalize_optional_hex(input.artifact_signature_hex, 64, "artifact_signature_hex")?;
    let artifact_signing_key_hex = normalize_optional_hex(
        input.artifact_signing_key_hex,
        32,
        "artifact_signing_key_hex",
    )?;
    let trusted_artifact_signing_key_hex = normalize_optional_hex(
        input.trusted_artifact_signing_key_hex,
        32,
        "trusted_artifact_signing_key_hex",
    )?;
    let job_id = input.job_id;
    let output = time::timeout(
        timeout,
        task::spawn_blocking(move || {
            stage_update_artifact(UpdateStageInput {
                job_id,
                artifact_url: &artifact_url,
                expected_sha256_hex: &sha256_hex,
                artifact_signature_hex: artifact_signature_hex.as_deref(),
                artifact_signing_key_hex: artifact_signing_key_hex.as_deref(),
                trusted_artifact_signing_key_hex: trusted_artifact_signing_key_hex.as_deref(),
                current_exe: &current_exe,
            })
        }),
    )
    .await
    .context("agent update staging timed out")?
    .context("agent update staging task failed")??;
    Ok(vec![output])
}

struct UpdateStageInput<'a> {
    job_id: uuid::Uuid,
    artifact_url: &'a str,
    expected_sha256_hex: &'a str,
    artifact_signature_hex: Option<&'a str>,
    artifact_signing_key_hex: Option<&'a str>,
    trusted_artifact_signing_key_hex: Option<&'a str>,
    current_exe: &'a Path,
}

fn stage_update_artifact(input: UpdateStageInput<'_>) -> Result<CommandOutput> {
    let artifact = fetch_update_artifact(input.artifact_url)?;
    let observed_sha256_hex = sha256_hex(&artifact);
    if observed_sha256_hex != input.expected_sha256_hex {
        anyhow::bail!(
            "agent update artifact hash mismatch: expected {}, got {observed_sha256_hex}",
            input.expected_sha256_hex
        );
    }
    let signature_status = verify_signature_policy(UpdateSignatureInput {
        expected_sha256_hex: input.expected_sha256_hex,
        artifact_signature_hex: input.artifact_signature_hex,
        artifact_signing_key_hex: input.artifact_signing_key_hex,
        trusted_artifact_signing_key_hex: input.trusted_artifact_signing_key_hex,
    })?;
    let staged_path = staged_path(input.current_exe)?;
    let rollback_path = rollback_path(input.current_exe)?;
    persist_staged_artifact(input.current_exe, &staged_path, &rollback_path, &artifact)?;
    let status = serde_json::json!({
        "type": "agent_update",
        "status": "staged",
        "sha256_hex": observed_sha256_hex,
        "size_bytes": artifact.len(),
        "signature": signature_status,
        "staged_path": staged_path.display().to_string(),
        "rollback_path": rollback_path.display().to_string(),
        "activation": "manual_restart_required",
    });
    Ok(CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    })
}

struct UpdateSignatureInput<'a> {
    expected_sha256_hex: &'a str,
    artifact_signature_hex: Option<&'a str>,
    artifact_signing_key_hex: Option<&'a str>,
    trusted_artifact_signing_key_hex: Option<&'a str>,
}

fn verify_signature_policy(input: UpdateSignatureInput<'_>) -> Result<&'static str> {
    match input.trusted_artifact_signing_key_hex {
        Some(trusted_key) => {
            let signature = input
                .artifact_signature_hex
                .context("trusted update artifact signing key requires artifact_signature_hex")?;
            let signing_key = input
                .artifact_signing_key_hex
                .context("trusted update artifact signing key requires artifact_signing_key_hex")?;
            if signing_key != trusted_key {
                anyhow::bail!("update artifact signing key does not match trusted key");
            }
            if !verify_update_artifact_signature(signing_key, signature, input.expected_sha256_hex)
            {
                anyhow::bail!("update artifact signature verification failed");
            }
            Ok("verified")
        }
        None => match (input.artifact_signature_hex, input.artifact_signing_key_hex) {
            (Some(signature), Some(signing_key)) => {
                if !verify_update_artifact_signature(
                    signing_key,
                    signature,
                    input.expected_sha256_hex,
                ) {
                    anyhow::bail!("update artifact signature verification failed");
                }
                Ok("verified_unpinned")
            }
            (None, None) => Ok("not_provided"),
            _ => anyhow::bail!(
                "artifact_signature_hex and artifact_signing_key_hex must be provided together"
            ),
        },
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
    fs::write(&temp_path, artifact)
        .with_context(|| format!("failed to write staged update {}", temp_path.display()))?;
    fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to set executable mode on {}", temp_path.display()))?;
    fs::rename(&temp_path, staged_path).with_context(|| {
        let _ = fs::remove_file(&temp_path);
        format!(
            "failed to atomically move staged update {} to {}",
            temp_path.display(),
            staged_path.display()
        )
    })?;
    Ok(())
}

fn fetch_update_artifact(artifact_url: &str) -> Result<Vec<u8>> {
    let parsed = parse_artifact_url(artifact_url)?;
    match parsed.scheme {
        ArtifactScheme::File => read_file_artifact(&parsed),
        ArtifactScheme::Https => fetch_http_artifact(&parsed, true),
        ArtifactScheme::HttpLocalDev => fetch_http_artifact(&parsed, false),
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

fn fetch_http_artifact(parsed: &ParsedArtifactUrl, tls: bool) -> Result<Vec<u8>> {
    let response = if tls {
        let tcp = connect_tcp(parsed)?;
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        load_extra_update_roots(&mut roots)?;
        let tls_config =
            ClientConfig::builder_with_provider(Arc::new(rustls_rustcrypto::provider()))
                .with_safe_default_protocol_versions()
                .context("failed to configure update TLS protocol versions")?
                .with_root_certificates(roots)
                .with_no_client_auth();
        let server_name = ServerName::try_from(parsed.host.clone())
            .context("update artifact URL host is not a valid TLS server name")?;
        let connection = ClientConnection::new(Arc::new(tls_config), server_name)
            .context("failed to create update TLS client")?;
        let mut stream = StreamOwned::new(connection, tcp);
        send_http_get(&mut stream, parsed)?;
        read_limited_response(&mut stream)?
    } else {
        let mut tcp = connect_tcp(parsed)?;
        send_http_get(&mut tcp, parsed)?;
        read_limited_response(&mut tcp)?
    };
    decode_http_response(&response)
}

fn load_extra_update_roots(roots: &mut RootCertStore) -> Result<()> {
    let Ok(path) = env::var(UPDATE_ROOT_CERT_PEM_ENV) else {
        return Ok(());
    };
    if path.trim().is_empty() {
        return Ok(());
    }
    let pem = fs::read_to_string(&path)
        .with_context(|| format!("failed to read update root certificate PEM from {path}"))?;
    let count = add_pem_certificates(roots, &pem)?;
    if count == 0 {
        anyhow::bail!("update root certificate PEM contained no certificates");
    }
    Ok(())
}

fn add_pem_certificates(roots: &mut RootCertStore, pem: &str) -> Result<usize> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";

    let mut rest = pem;
    let mut count = 0usize;
    while let Some(begin) = rest.find(BEGIN) {
        let after_begin = &rest[begin + BEGIN.len()..];
        let end = after_begin
            .find(END)
            .context("update root certificate PEM is missing END CERTIFICATE marker")?;
        let base64_body = after_begin[..end]
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<String>();
        let der = BASE64_STANDARD
            .decode(base64_body.as_bytes())
            .context("update root certificate PEM is not valid base64")?;
        roots
            .add(CertificateDer::from(der))
            .context("failed to add update root certificate")?;
        count += 1;
        rest = &after_begin[end + END.len()..];
    }
    Ok(count)
}

fn connect_tcp(parsed: &ParsedArtifactUrl) -> Result<StdTcpStream> {
    let mut last_error = None;
    for addr in (parsed.host.as_str(), parsed.port).to_socket_addrs()? {
        match StdTcpStream::connect_timeout(&addr, UPDATE_DOWNLOAD_TIMEOUT) {
            Ok(stream) => {
                stream.set_read_timeout(Some(UPDATE_DOWNLOAD_TIMEOUT))?;
                stream.set_write_timeout(Some(UPDATE_DOWNLOAD_TIMEOUT))?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .map(anyhow::Error::new)
        .unwrap_or_else(|| anyhow!("update artifact host resolved to no addresses")))
}

fn send_http_get(stream: &mut impl Write, parsed: &ParsedArtifactUrl) -> Result<()> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: vpsman-agent/{}\r\nAccept: application/octet-stream\r\nConnection: close\r\n\r\n",
        parsed.path_and_query,
        parsed.host_header(),
        env!("CARGO_PKG_VERSION")
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn read_limited_response(stream: &mut impl Read) -> Result<Vec<u8>> {
    let mut response = Vec::new();
    stream
        .take((MAX_UPDATE_ARTIFACT_BYTES + 4096) as u64)
        .read_to_end(&mut response)?;
    if response.len() > MAX_UPDATE_ARTIFACT_BYTES + 4096 {
        anyhow::bail!("update artifact HTTP response exceeded size limit");
    }
    Ok(response)
}

fn decode_http_response(response: &[u8]) -> Result<Vec<u8>> {
    let header_end = find_bytes(response, b"\r\n\r\n").context("update HTTP header incomplete")?;
    let header =
        std::str::from_utf8(&response[..header_end]).context("update HTTP header is not UTF-8")?;
    let body = &response[header_end + 4..];
    let mut lines = header.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("update HTTP response missing status code")?;
    if status != "200" {
        anyhow::bail!("update artifact endpoint returned HTTP {status}");
    }
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("transfer-encoding")
            && value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        {
            chunked = true;
        }
    }
    let decoded = if chunked {
        decode_chunked_body(body)?
    } else {
        body.to_vec()
    };
    if decoded.len() > MAX_UPDATE_ARTIFACT_BYTES {
        anyhow::bail!("update artifact exceeds {MAX_UPDATE_ARTIFACT_BYTES} bytes");
    }
    Ok(decoded)
}

fn decode_chunked_body(mut body: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let size_end = find_bytes(body, b"\r\n").context("chunked update body is incomplete")?;
        let size_line = std::str::from_utf8(&body[..size_end])
            .context("chunked update size line is not UTF-8")?;
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16)
            .context("chunked update body has invalid chunk size")?;
        body = &body[size_end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        if body.len() < size + 2 || &body[size..size + 2] != b"\r\n" {
            anyhow::bail!("chunked update body chunk is incomplete");
        }
        decoded.extend_from_slice(&body[..size]);
        if decoded.len() > MAX_UPDATE_ARTIFACT_BYTES {
            anyhow::bail!("decoded update artifact exceeded {MAX_UPDATE_ARTIFACT_BYTES} bytes");
        }
        body = &body[size + 2..];
    }
}

fn normalize_sha256(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() != 64 || !value.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        anyhow::bail!("sha256_hex must be 64 lowercase or uppercase hex characters");
    }
    Ok(value)
}

fn normalize_optional_hex(
    value: Option<&str>,
    byte_len: usize,
    field: &str,
) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_ascii_lowercase();
    if value.len() != byte_len * 2 || !value.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        anyhow::bail!("{field} must be {byte_len} byte hex");
    }
    Ok(Some(value))
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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
    fn host_header(&self) -> String {
        let default_port = match self.scheme {
            ArtifactScheme::File => 0,
            ArtifactScheme::Https => 443,
            ArtifactScheme::HttpLocalDev => 80,
        };
        if self.port == default_port {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
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

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::{
        add_pem_certificates, decode_http_response, normalize_sha256, parse_artifact_url,
        sha256_hex, stage_update_artifact, UpdateStageInput,
    };
    use ed25519_dalek::SigningKey;
    use rustls::RootCertStore;
    use vpsman_common::sign_update_artifact_hash;

    #[test]
    fn parses_artifact_urls_and_rejects_remote_http() {
        assert!(parse_artifact_url("https://updates.example/vpsman-agent").is_ok());
        assert!(parse_artifact_url("http://127.0.0.1:8080/vpsman-agent").is_ok());
        assert!(parse_artifact_url("file:///tmp/vpsman-agent").is_ok());
        assert!(parse_artifact_url("http://updates.example/vpsman-agent").is_err());
    }

    #[test]
    fn normalizes_sha256_hex() {
        assert_eq!(normalize_sha256(&"AA".repeat(32)).unwrap(), "aa".repeat(32));
        assert!(normalize_sha256("not-a-hash").is_err());
    }

    #[test]
    fn stages_file_artifact_after_hash_verification() {
        let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let current = dir.join("vpsman-agent");
        let artifact = dir.join("vpsman-agent-new");
        fs::write(&current, b"old-agent").unwrap();
        fs::write(&artifact, b"new-agent").unwrap();
        let hash = sha256_hex(b"new-agent");
        let output = stage_update_artifact(UpdateStageInput {
            job_id: uuid::Uuid::new_v4(),
            artifact_url: &format!("file://{}", artifact.display()),
            expected_sha256_hex: &hash,
            artifact_signature_hex: None,
            artifact_signing_key_hex: None,
            trusted_artifact_signing_key_hex: None,
            current_exe: &current,
        })
        .unwrap();

        let staged = dir.join("vpsman-agent.next");
        let rollback = dir.join("vpsman-agent.rollback");
        assert_eq!(fs::read(staged).unwrap(), b"new-agent");
        assert_eq!(fs::read(rollback).unwrap(), b"old-agent");
        assert_eq!(
            fs::metadata(dir.join("vpsman-agent.next"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();
        assert_eq!(status["status"], "staged");
        assert_eq!(status["signature"], "not_provided");
        assert!(status.get("artifact_url").is_none());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_hash_mismatch_before_writing_staged_artifact() {
        let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let current = dir.join("vpsman-agent");
        let artifact = dir.join("vpsman-agent-new");
        fs::write(&current, b"old-agent").unwrap();
        fs::write(&artifact, b"new-agent").unwrap();

        assert!(stage_update_artifact(UpdateStageInput {
            job_id: uuid::Uuid::new_v4(),
            artifact_url: &format!("file://{}", artifact.display()),
            expected_sha256_hex: &"00".repeat(32),
            artifact_signature_hex: None,
            artifact_signing_key_hex: None,
            trusted_artifact_signing_key_hex: None,
            current_exe: &current,
        })
        .is_err());
        assert!(!dir.join("vpsman-agent.next").exists());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn verifies_signed_artifact_before_staging_when_trusted_key_is_pinned() {
        let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let current = dir.join("vpsman-agent");
        let artifact = dir.join("vpsman-agent-new");
        fs::write(&current, b"old-agent").unwrap();
        fs::write(&artifact, b"signed-agent").unwrap();
        let hash = sha256_hex(b"signed-agent");
        let signing_key = SigningKey::from_bytes(&[41_u8; 32]);
        let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
        let signature_hex = hex::encode(sign_update_artifact_hash(&signing_key, &hash));

        let output = stage_update_artifact(UpdateStageInput {
            job_id: uuid::Uuid::new_v4(),
            artifact_url: &format!("file://{}", artifact.display()),
            expected_sha256_hex: &hash,
            artifact_signature_hex: Some(&signature_hex),
            artifact_signing_key_hex: Some(&public_key_hex),
            trusted_artifact_signing_key_hex: Some(&public_key_hex),
            current_exe: &current,
        })
        .unwrap();
        let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();
        assert_eq!(status["signature"], "verified");

        assert!(stage_update_artifact(UpdateStageInput {
            job_id: uuid::Uuid::new_v4(),
            artifact_url: &format!("file://{}", artifact.display()),
            expected_sha256_hex: &hash,
            artifact_signature_hex: Some(&signature_hex),
            artifact_signing_key_hex: Some(&public_key_hex),
            trusted_artifact_signing_key_hex: Some(&"00".repeat(32)),
            current_exe: &current,
        })
        .is_err());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn decodes_chunked_update_response_before_hashing() {
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nnew\r\n6\r\n-agent\r\n0\r\n\r\n";
        assert_eq!(decode_http_response(response).unwrap(), b"new-agent");
    }

    #[test]
    fn rejects_incomplete_chunked_update_response() {
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nnew";
        assert!(decode_http_response(response).is_err());
    }

    #[test]
    fn rejects_invalid_extra_update_root_pem() {
        let mut roots = RootCertStore::empty();
        assert!(add_pem_certificates(
            &mut roots,
            "-----BEGIN CERTIFICATE-----\nnot-base64\n-----END CERTIFICATE-----"
        )
        .is_err());
    }
}
