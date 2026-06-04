use std::{
    io::{Read, Write},
    net::{TcpStream as StdTcpStream, ToSocketAddrs},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::VerifyingKey;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use rustls_pki_types::ServerName;
use tokio::task;
use vpsman_common::{
    verify_discovery_document_signature, AgentConfig, DiscoveryDocument, ServerEndpoint,
};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DISCOVERY_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_DISCOVERY_ENDPOINTS: usize = 32;
const MAX_ENDPOINT_LABEL_BYTES: usize = 64;
const MAX_ENDPOINT_ADDR_BYTES: usize = 256;

pub(crate) async fn refresh_discovery_endpoints(
    config: &AgentConfig,
) -> Result<Vec<ServerEndpoint>> {
    let url = config
        .discovery_url
        .clone()
        .context("agent discovery_url is not configured")?;
    let signature_required = discovery_signature_required(&url)?;
    let document = task::spawn_blocking(move || fetch_discovery_document(&url))
        .await
        .context("discovery fetch task failed")??;
    validate_discovery_document(
        document,
        unix_now()?,
        &discovery_trust_keys(config),
        signature_required,
    )
}

pub(crate) fn endpoint_candidates(
    config: &AgentConfig,
    discovered: &[ServerEndpoint],
) -> Vec<ServerEndpoint> {
    let mut endpoints = config.tcp_endpoints.clone();
    endpoints.extend_from_slice(discovered);
    sort_and_dedupe_endpoints(endpoints)
}

fn fetch_discovery_document(url: &str) -> Result<DiscoveryDocument> {
    let parsed = parse_discovery_url(url)?;
    let response = match parsed.scheme {
        DiscoveryScheme::Https => fetch_https(&parsed)?,
        DiscoveryScheme::HttpLocalDev => fetch_http(&parsed)?,
    };
    let body = decode_http_response(&response)?;
    serde_json::from_slice(&body).context("failed to parse discovery document JSON")
}

fn fetch_https(parsed: &ParsedDiscoveryUrl) -> Result<Vec<u8>> {
    let tcp = connect_tcp(parsed)?;
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = ClientConfig::builder_with_provider(Arc::new(rustls_rustcrypto::provider()))
        .with_safe_default_protocol_versions()
        .context("failed to configure discovery TLS protocol versions")?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = ServerName::try_from(parsed.host.clone())
        .context("discovery URL host is not a valid TLS server name")?;
    let connection = ClientConnection::new(Arc::new(tls_config), server_name)
        .context("failed to create discovery TLS client")?;
    let mut stream = StreamOwned::new(connection, tcp);
    send_http_get(&mut stream, parsed)?;
    read_limited_response(&mut stream)
}

fn fetch_http(parsed: &ParsedDiscoveryUrl) -> Result<Vec<u8>> {
    let mut tcp = connect_tcp(parsed)?;
    send_http_get(&mut tcp, parsed)?;
    read_limited_response(&mut tcp)
}

fn connect_tcp(parsed: &ParsedDiscoveryUrl) -> Result<StdTcpStream> {
    let mut last_error = None;
    for addr in (parsed.host.as_str(), parsed.port).to_socket_addrs()? {
        match StdTcpStream::connect_timeout(&addr, DISCOVERY_TIMEOUT) {
            Ok(stream) => {
                stream.set_read_timeout(Some(DISCOVERY_TIMEOUT))?;
                stream.set_write_timeout(Some(DISCOVERY_TIMEOUT))?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .map(anyhow::Error::new)
        .unwrap_or_else(|| anyhow!("discovery URL host resolved to no addresses")))
}

fn send_http_get(stream: &mut impl Write, parsed: &ParsedDiscoveryUrl) -> Result<()> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: vpsman-agent/{}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
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
        .take((MAX_DISCOVERY_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response)?;
    if response.len() > MAX_DISCOVERY_RESPONSE_BYTES {
        anyhow::bail!("discovery response exceeded {MAX_DISCOVERY_RESPONSE_BYTES} bytes");
    }
    Ok(response)
}

fn decode_http_response(response: &[u8]) -> Result<Vec<u8>> {
    let header_end =
        find_bytes(response, b"\r\n\r\n").context("discovery HTTP header incomplete")?;
    let header = std::str::from_utf8(&response[..header_end])
        .context("discovery HTTP header is not UTF-8")?;
    let body = &response[header_end + 4..];
    let mut lines = header.split("\r\n");
    let status_line = lines
        .next()
        .context("discovery HTTP response missing status line")?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .context("discovery HTTP response missing status code")?;
    if status != "200" {
        anyhow::bail!("discovery endpoint returned HTTP {status}");
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

    if chunked {
        return decode_chunked_body(body);
    }
    Ok(body.to_vec())
}

fn decode_chunked_body(mut body: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let size_end = find_bytes(body, b"\r\n").context("chunked discovery body is incomplete")?;
        let size_line = std::str::from_utf8(&body[..size_end])
            .context("chunked discovery size line is not UTF-8")?;
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16)
            .context("chunked discovery body has invalid chunk size")?;
        body = &body[size_end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        if body.len() < size + 2 || &body[size..size + 2] != b"\r\n" {
            anyhow::bail!("chunked discovery body chunk is incomplete");
        }
        decoded.extend_from_slice(&body[..size]);
        if decoded.len() > MAX_DISCOVERY_RESPONSE_BYTES {
            anyhow::bail!("decoded discovery body exceeded {MAX_DISCOVERY_RESPONSE_BYTES} bytes");
        }
        body = &body[size + 2..];
    }
}

fn validate_discovery_document(
    document: DiscoveryDocument,
    now_unix: u64,
    trusted_server_public_key_hexes: &[String],
    signature_required: bool,
) -> Result<Vec<ServerEndpoint>> {
    if document.version != 1 {
        anyhow::bail!(
            "unsupported discovery document version {}",
            document.version
        );
    }
    if document.expires_unix <= now_unix {
        anyhow::bail!("discovery document is expired");
    }
    if document.issued_unix > now_unix + 300 {
        anyhow::bail!("discovery document issued time is too far in the future");
    }
    if signature_required && trusted_server_public_key_hexes.is_empty() {
        anyhow::bail!(
            "discovery document signature is required but no trusted server discovery key is configured"
        );
    }
    if signature_required || !trusted_server_public_key_hexes.is_empty() {
        verify_document_signature(trusted_server_public_key_hexes, &document)?;
    }
    validate_endpoint_list(&document.endpoints)?;
    Ok(sort_and_dedupe_endpoints(document.endpoints))
}

fn verify_document_signature(
    trusted_server_public_key_hexes: &[String],
    document: &DiscoveryDocument,
) -> Result<()> {
    if document.signature.is_empty() {
        anyhow::bail!("discovery document is missing server signature");
    }
    for server_public_key_hex in trusted_server_public_key_hexes {
        let public_key = hex::decode(server_public_key_hex.trim())
            .context("invalid server discovery signing public key hex")?;
        let public_key: [u8; 32] = public_key
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("server discovery signing public key must be 32 bytes"))?;
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .context("invalid server discovery signing public key")?;
        if verify_discovery_document_signature(&verifying_key, document) {
            return Ok(());
        }
    }
    anyhow::bail!("discovery document server signature is invalid")
}

fn discovery_trust_keys(config: &AgentConfig) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(key) = config
        .auth
        .server_ed25519_public_key_hex
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        keys.push(key.to_ascii_lowercase());
    }
    for key in &config.auth.discovery_trusted_server_ed25519_public_keys_hex {
        let key = key.trim();
        if !key.is_empty() {
            keys.push(key.to_ascii_lowercase());
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

fn validate_endpoint_list(endpoints: &[ServerEndpoint]) -> Result<()> {
    if endpoints.is_empty() {
        anyhow::bail!("discovery document has no endpoints");
    }
    if endpoints.len() > MAX_DISCOVERY_ENDPOINTS {
        anyhow::bail!("discovery document has too many endpoints");
    }
    for endpoint in endpoints {
        validate_endpoint(endpoint)?;
    }
    Ok(())
}

fn validate_endpoint(endpoint: &ServerEndpoint) -> Result<()> {
    let label = endpoint.label.trim();
    let addr = endpoint.tcp_addr.trim();
    if label.is_empty() || label.len() > MAX_ENDPOINT_LABEL_BYTES || contains_control(label) {
        anyhow::bail!("discovery endpoint has invalid label");
    }
    if addr.is_empty() || addr.len() > MAX_ENDPOINT_ADDR_BYTES || contains_control(addr) {
        anyhow::bail!("discovery endpoint has invalid TCP address");
    }
    if !addr.contains(':') {
        anyhow::bail!("discovery endpoint TCP address must include host and port");
    }
    Ok(())
}

fn sort_and_dedupe_endpoints(mut endpoints: Vec<ServerEndpoint>) -> Vec<ServerEndpoint> {
    endpoints.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.tcp_addr.cmp(&right.tcp_addr))
    });
    let mut deduped = Vec::new();
    for endpoint in endpoints {
        if deduped
            .iter()
            .any(|existing: &ServerEndpoint| existing.tcp_addr == endpoint.tcp_addr)
        {
            continue;
        }
        deduped.push(endpoint);
    }
    deduped
}

fn contains_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_secs())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiscoveryScheme {
    Https,
    HttpLocalDev,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedDiscoveryUrl {
    scheme: DiscoveryScheme,
    host: String,
    port: u16,
    path_and_query: String,
}

impl ParsedDiscoveryUrl {
    fn host_header(&self) -> String {
        let default_port = match self.scheme {
            DiscoveryScheme::Https => 443,
            DiscoveryScheme::HttpLocalDev => 80,
        };
        if self.port == default_port {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

fn parse_discovery_url(url: &str) -> Result<ParsedDiscoveryUrl> {
    let trimmed = url.trim();
    if trimmed.contains('#') {
        anyhow::bail!("discovery URL fragments are not supported");
    }
    let (scheme, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
        (DiscoveryScheme::Https, rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        (DiscoveryScheme::HttpLocalDev, rest)
    } else {
        anyhow::bail!("discovery URL must use https://");
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let suffix = &rest[authority_end..];
    if authority.is_empty() || authority.contains('@') {
        anyhow::bail!("discovery URL authority is invalid");
    }

    let (host, port) = parse_authority(authority, scheme)?;
    if scheme == DiscoveryScheme::HttpLocalDev && !is_localhost(&host) {
        anyhow::bail!("http:// discovery URLs are allowed only for localhost development");
    }

    let path_and_query = if suffix.is_empty() {
        "/".to_string()
    } else if suffix.starts_with('?') {
        format!("/{suffix}")
    } else {
        suffix.to_string()
    };
    Ok(ParsedDiscoveryUrl {
        scheme,
        host,
        port,
        path_and_query,
    })
}

fn discovery_signature_required(url: &str) -> Result<bool> {
    Ok(parse_discovery_url(url)?.scheme == DiscoveryScheme::Https)
}

fn parse_authority(authority: &str, scheme: DiscoveryScheme) -> Result<(String, u16)> {
    let default_port = match scheme {
        DiscoveryScheme::Https => 443,
        DiscoveryScheme::HttpLocalDev => 80,
    };
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .context("invalid bracketed IPv6 discovery URL host")?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix
                .strip_prefix(':')
                .context("invalid bracketed IPv6 discovery URL port")?
                .parse::<u16>()
                .context("invalid discovery URL port")?
        };
        return Ok((host.to_string(), port));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (
            host,
            port.parse::<u16>().context("invalid discovery URL port")?,
        ),
        _ => (authority, default_port),
    };
    if host.trim().is_empty() {
        anyhow::bail!("discovery URL host is empty");
    }
    Ok((host.to_string(), port))
}

fn is_localhost(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
