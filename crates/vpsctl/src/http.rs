use std::{
    fs::File,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use rustls_pki_types::ServerName;

const API_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_API_RESPONSE_BYTES: usize = 24 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApiScheme {
    Http,
    Https,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedApiUrl {
    scheme: ApiScheme,
    host: String,
    port: u16,
    prefix: String,
}

impl ParsedApiUrl {
    fn host_header(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default_port = match self.scheme {
            ApiScheme::Http => 80,
            ApiScheme::Https => 443,
        };
        if self.port == default_port {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    fn request_path(&self, path: &str) -> String {
        format!("{}{}", self.prefix, path)
    }
}

pub(crate) fn http_get(base_url: &str, path: &str, bearer_token: Option<&str>) -> Result<String> {
    http_request(base_url, "GET", path, bearer_token, None)
}

pub(crate) fn http_get_bytes(
    base_url: &str,
    path: &str,
    bearer_token: Option<&str>,
) -> Result<Vec<u8>> {
    http_request_bytes(base_url, "GET", path, bearer_token, None)
}

pub(crate) fn http_delete(
    base_url: &str,
    path: &str,
    bearer_token: Option<&str>,
) -> Result<String> {
    http_request(base_url, "DELETE", path, bearer_token, None)
}

pub(crate) fn http_delete_json(
    base_url: &str,
    path: &str,
    bearer_token: Option<&str>,
    value: &serde_json::Value,
) -> Result<String> {
    http_request(
        base_url,
        "DELETE",
        path,
        bearer_token,
        Some(serde_json::to_vec(value)?),
    )
}

pub(crate) fn http_post_json(
    base_url: &str,
    path: &str,
    bearer_token: Option<&str>,
    value: &serde_json::Value,
) -> Result<String> {
    http_request(
        base_url,
        "POST",
        path,
        bearer_token,
        Some(serde_json::to_vec(value)?),
    )
}

pub(crate) fn http_put_json(
    base_url: &str,
    path: &str,
    bearer_token: Option<&str>,
    value: &serde_json::Value,
) -> Result<String> {
    http_request(
        base_url,
        "PUT",
        path,
        bearer_token,
        Some(serde_json::to_vec(value)?),
    )
}

pub(crate) fn http_post_file(
    base_url: &str,
    path: &str,
    bearer_token: Option<&str>,
    file_path: &Path,
    content_type: &str,
    extra_headers: &[(&str, String)],
) -> Result<String> {
    let response = http_request_file(
        base_url,
        "POST",
        path,
        bearer_token,
        file_path,
        content_type,
        extra_headers,
    )?;
    Ok(String::from_utf8_lossy(&response).trim().to_string())
}

fn http_request(
    base_url: &str,
    method: &str,
    path: &str,
    bearer_token: Option<&str>,
    body: Option<Vec<u8>>,
) -> Result<String> {
    let response = http_request_bytes(base_url, method, path, bearer_token, body)?;
    Ok(String::from_utf8_lossy(&response).trim().to_string())
}

fn http_request_bytes(
    base_url: &str,
    method: &str,
    path: &str,
    bearer_token: Option<&str>,
    body: Option<Vec<u8>>,
) -> Result<Vec<u8>> {
    let parsed = parse_api_url(base_url)?;
    let request_path = parsed.request_path(path);
    let body = body.unwrap_or_default();
    let request = build_request(
        method,
        &request_path,
        &parsed.host_header(),
        bearer_token,
        &body,
        "application/json",
        &[],
    );
    let response = match parsed.scheme {
        ApiScheme::Http => {
            let mut stream = connect_tcp(&parsed)?;
            write_request_and_read_response(&mut stream, &request, &body)?
        }
        ApiScheme::Https => {
            let tcp = connect_tcp(&parsed)?;
            let mut stream = tls_stream(tcp, &parsed)?;
            write_request_and_read_response(&mut stream, &request, &body)?
        }
    };
    decode_api_response_bytes(method, &request_path, &response)
}

fn http_request_file(
    base_url: &str,
    method: &str,
    path: &str,
    bearer_token: Option<&str>,
    file_path: &Path,
    content_type: &str,
    extra_headers: &[(&str, String)],
) -> Result<Vec<u8>> {
    let parsed = parse_api_url(base_url)?;
    let request_path = parsed.request_path(path);
    let metadata = std::fs::metadata(file_path)
        .with_context(|| format!("failed to stat upload file {}", file_path.display()))?;
    anyhow::ensure!(metadata.is_file(), "upload path is not a file");
    let request = build_request_with_len(
        method,
        &request_path,
        &parsed.host_header(),
        bearer_token,
        metadata.len(),
        content_type,
        extra_headers,
    )?;
    let response = match parsed.scheme {
        ApiScheme::Http => {
            let mut stream = connect_tcp(&parsed)?;
            write_request_file_and_read_response(&mut stream, &request, file_path)?
        }
        ApiScheme::Https => {
            let tcp = connect_tcp(&parsed)?;
            let mut stream = tls_stream(tcp, &parsed)?;
            write_request_file_and_read_response(&mut stream, &request, file_path)?
        }
    };
    decode_api_response_bytes(method, &request_path, &response)
}

fn build_request(
    method: &str,
    request_path: &str,
    host_header: &str,
    bearer_token: Option<&str>,
    body: &[u8],
    content_type: &str,
    extra_headers: &[(&str, String)],
) -> String {
    build_request_with_len(
        method,
        request_path,
        host_header,
        bearer_token,
        body.len() as u64,
        content_type,
        extra_headers,
    )
    .expect("static HTTP request headers are valid")
}

fn build_request_with_len(
    method: &str,
    request_path: &str,
    host_header: &str,
    bearer_token: Option<&str>,
    content_len: u64,
    content_type: &str,
    extra_headers: &[(&str, String)],
) -> Result<String> {
    let mut request = format!(
        "{method} {request_path} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: vpsctl/{} build/{}\r\nConnection: close\r\nAccept: application/json\r\n",
        env!("CARGO_PKG_VERSION"),
        crate::build_info::CLI_BUILD_NUMBER
    );
    if let Some(token) = bearer_token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if content_len > 0 {
        validate_header_value(content_type)?;
        request.push_str(&format!("Content-Type: {content_type}\r\n"));
        request.push_str(&format!("Content-Length: {content_len}\r\n"));
    }
    for (name, value) in extra_headers {
        validate_header_name(name)?;
        validate_header_value(value)?;
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    Ok(request)
}

fn connect_tcp(parsed: &ParsedApiUrl) -> Result<TcpStream> {
    let mut last_error = None;
    for addr in (parsed.host.as_str(), parsed.port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, API_HTTP_TIMEOUT) {
            Ok(stream) => {
                stream.set_read_timeout(Some(API_HTTP_TIMEOUT))?;
                stream.set_write_timeout(Some(API_HTTP_TIMEOUT))?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .map(anyhow::Error::new)
        .unwrap_or_else(|| anyhow!("API host resolved to no addresses")))
}

fn tls_stream(
    tcp: TcpStream,
    parsed: &ParsedApiUrl,
) -> Result<StreamOwned<ClientConnection, TcpStream>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = ClientConfig::builder_with_provider(Arc::new(rustls_rustcrypto::provider()))
        .with_safe_default_protocol_versions()
        .context("failed to configure API TLS protocol versions")?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = ServerName::try_from(parsed.host.clone())
        .context("API URL host is not a valid TLS server name")?;
    let connection = ClientConnection::new(Arc::new(tls_config), server_name)
        .context("failed to create API TLS client")?;
    Ok(StreamOwned::new(connection, tcp))
}

fn write_request_and_read_response(
    stream: &mut (impl Read + Write),
    request: &str,
    body: &[u8],
) -> Result<Vec<u8>> {
    stream.write_all(request.as_bytes())?;
    if !body.is_empty() {
        stream.write_all(body)?;
    }
    stream.flush()?;

    let mut response = Vec::new();
    stream
        .take((MAX_API_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response)?;
    if response.len() > MAX_API_RESPONSE_BYTES {
        anyhow::bail!("API response exceeded {MAX_API_RESPONSE_BYTES} bytes");
    }
    Ok(response)
}

fn write_request_file_and_read_response(
    stream: &mut (impl Read + Write),
    request: &str,
    file_path: &Path,
) -> Result<Vec<u8>> {
    stream.write_all(request.as_bytes())?;
    let mut file =
        File::open(file_path).with_context(|| format!("failed to open {}", file_path.display()))?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        stream.write_all(&buffer[..read])?;
    }
    stream.flush()?;

    let mut response = Vec::new();
    stream
        .take((MAX_API_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response)?;
    if response.len() > MAX_API_RESPONSE_BYTES {
        anyhow::bail!("API response exceeded {MAX_API_RESPONSE_BYTES} bytes");
    }
    Ok(response)
}

fn validate_header_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid HTTP header name"
    );
    Ok(())
}

fn validate_header_value(value: &str) -> Result<()> {
    anyhow::ensure!(
        !value.contains('\r') && !value.contains('\n') && !value.as_bytes().contains(&0),
        "invalid HTTP header value"
    );
    Ok(())
}

#[cfg(test)]
fn decode_api_response(method: &str, request_path: &str, response: &[u8]) -> Result<String> {
    let body = decode_api_response_bytes(method, request_path, response)?;
    Ok(String::from_utf8_lossy(&body).trim().to_string())
}

fn decode_api_response_bytes(method: &str, request_path: &str, response: &[u8]) -> Result<Vec<u8>> {
    let header_end = find_bytes(response, b"\r\n\r\n").context("invalid HTTP response from API")?;
    let head =
        std::str::from_utf8(&response[..header_end]).context("API response header is not UTF-8")?;
    let body = &response[header_end + 4..];
    let status_line = head.lines().next().context("missing HTTP status line")?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .context("missing HTTP status code")?;
    anyhow::ensure!(
        status_code.starts_with('2'),
        "API request {method} {request_path} failed: {status_line}: {}",
        String::from_utf8_lossy(body).trim()
    );
    Ok(body.to_vec())
}

fn parse_api_url(base_url: &str) -> Result<ParsedApiUrl> {
    let trimmed = base_url.trim();
    if trimmed.contains('#') {
        anyhow::bail!("API URL fragments are not supported");
    }
    let (scheme, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
        (ApiScheme::Https, rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        (ApiScheme::Http, rest)
    } else {
        anyhow::bail!("API URL must use http:// or https://");
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let suffix = &rest[authority_end..];
    if authority.is_empty() || authority.contains('@') {
        anyhow::bail!("API URL authority is invalid");
    }
    let (host, port) = parse_authority(authority, scheme)?;
    let prefix = if suffix.is_empty() {
        String::new()
    } else if suffix.starts_with('?') {
        format!("/{suffix}")
    } else {
        suffix.trim_end_matches('/').to_string()
    };
    Ok(ParsedApiUrl {
        scheme,
        host,
        port,
        prefix,
    })
}

fn parse_authority(authority: &str, scheme: ApiScheme) -> Result<(String, u16)> {
    let default_port = match scheme {
        ApiScheme::Http => 80,
        ApiScheme::Https => 443,
    };
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .context("invalid bracketed IPv6 API URL host")?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix
                .strip_prefix(':')
                .context("invalid bracketed IPv6 API URL port")?
                .parse::<u16>()
                .context("invalid API URL port")?
        };
        return validate_host_and_port(host, port);
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            (host, port.parse::<u16>().context("invalid API URL port")?)
        }
        _ => (authority, default_port),
    };
    validate_host_and_port(host, port)
}

fn validate_host_and_port(host: &str, port: u16) -> Result<(String, u16)> {
    if host.trim().is_empty() || host.as_bytes().contains(&0) || port == 0 {
        anyhow::bail!("API URL authority is invalid");
    }
    Ok((host.to_string(), port))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_and_https_api_urls() {
        let http = parse_api_url("http://127.0.0.1:8080").unwrap();
        assert_eq!(http.scheme, ApiScheme::Http);
        assert_eq!(http.host, "127.0.0.1");
        assert_eq!(http.port, 8080);
        assert_eq!(http.prefix, "");
        assert_eq!(http.request_path("/api/v1/health"), "/api/v1/health");
        assert_eq!(http.host_header(), "127.0.0.1:8080");

        let https = parse_api_url("https://panel.example.com/vpsman/").unwrap();
        assert_eq!(https.scheme, ApiScheme::Https);
        assert_eq!(https.host, "panel.example.com");
        assert_eq!(https.port, 443);
        assert_eq!(https.prefix, "/vpsman");
        assert_eq!(
            https.request_path("/api/v1/health"),
            "/vpsman/api/v1/health"
        );
        assert_eq!(https.host_header(), "panel.example.com");
    }

    #[test]
    fn parses_ipv6_api_url_with_host_header() {
        let parsed = parse_api_url("https://[2001:db8::1]:8443/base").unwrap();
        assert_eq!(parsed.host, "2001:db8::1");
        assert_eq!(parsed.port, 8443);
        assert_eq!(parsed.host_header(), "[2001:db8::1]:8443");
    }

    #[test]
    fn rejects_unsupported_or_unsafe_api_urls() {
        assert!(parse_api_url("ftp://panel.example.com").is_err());
        assert!(parse_api_url("https://user@panel.example.com").is_err());
        assert!(parse_api_url("https://panel.example.com/#fragment").is_err());
        assert!(parse_api_url("https://:443").is_err());
    }

    #[test]
    fn decodes_success_and_failure_responses() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"ok\":true}\n";
        assert_eq!(
            decode_api_response("GET", "/api/v1/health", ok).unwrap(),
            "{\"ok\":true}"
        );

        let err = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 11\r\n\r\nnot allowed";
        let message = decode_api_response("POST", "/api/v1/jobs", err)
            .unwrap_err()
            .to_string();
        assert!(message.contains("403 Forbidden"));
        assert!(message.contains("not allowed"));
    }
}
