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
const API_BINARY_HTTP_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_API_RESPONSE_BYTES: usize = 24 * 1024 * 1024;
const MAX_API_RESPONSE_HEADER_BYTES: usize = 64 * 1024;

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

pub(crate) fn http_get_to_file(
    base_url: &str,
    path: &str,
    bearer_token: Option<&str>,
    output_file: &Path,
) -> Result<u64> {
    let parsed = parse_api_url(base_url)?;
    let request_path = parsed.request_path(path);
    let request = build_request(
        "GET",
        &request_path,
        &parsed.host_header(),
        bearer_token,
        &[],
        "application/json",
        &[],
    );
    match parsed.scheme {
        ApiScheme::Http => {
            let mut stream = connect_tcp_with_timeout(&parsed, API_BINARY_HTTP_TIMEOUT)?;
            write_request_and_stream_response_to_file(
                &mut stream,
                &request,
                "GET",
                &request_path,
                output_file,
            )
        }
        ApiScheme::Https => {
            let tcp = connect_tcp_with_timeout(&parsed, API_BINARY_HTTP_TIMEOUT)?;
            let mut stream = tls_stream(tcp, &parsed)?;
            write_request_and_stream_response_to_file(
                &mut stream,
                &request,
                "GET",
                &request_path,
                output_file,
            )
        }
    }
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
    connect_tcp_with_timeout(parsed, API_HTTP_TIMEOUT)
}

fn connect_tcp_with_timeout(parsed: &ParsedApiUrl, timeout: Duration) -> Result<TcpStream> {
    let mut last_error = None;
    for addr in (parsed.host.as_str(), parsed.port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
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

fn write_request_and_stream_response_to_file(
    stream: &mut (impl Read + Write),
    request: &str,
    method: &str,
    request_path: &str,
    output_file: &Path,
) -> Result<u64> {
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let headers = read_response_headers(stream)?;
    let header_end = find_bytes(&headers, b"\r\n\r\n").context("invalid HTTP response from API")?;
    let head =
        std::str::from_utf8(&headers[..header_end]).context("API response header is not UTF-8")?;
    let status_line = head.lines().next().context("missing HTTP status line")?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .context("missing HTTP status code")?;
    if !status_code.starts_with('2') {
        let mut body = Vec::new();
        stream
            .take((MAX_API_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut body)?;
        anyhow::ensure!(
            body.len() <= MAX_API_RESPONSE_BYTES,
            "API error response exceeded {MAX_API_RESPONSE_BYTES} bytes"
        );
        anyhow::bail!(
            "API request {method} {request_path} failed: {status_line}: {}",
            String::from_utf8_lossy(&body).trim()
        );
    }

    let temp_path = download_temp_path(output_file);
    let mut file = File::create(&temp_path)
        .with_context(|| format!("failed to create {}", temp_path.display()))?;
    let transfer_encoding = header_value(head, "transfer-encoding").unwrap_or_default();
    let result = if transfer_encoding
        .to_ascii_lowercase()
        .split(',')
        .any(|value| value.trim() == "chunked")
    {
        copy_chunked_body_to_file(stream, &mut file)
    } else if let Some(content_length) = header_value(head, "content-length") {
        let content_length = content_length
            .trim()
            .parse::<u64>()
            .context("API response content-length is invalid")?;
        copy_exact_body_to_file(stream, &mut file, content_length)
    } else {
        copy_body_to_file(stream, &mut file)
    };
    let written = match result {
        Ok(written) => written,
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(error);
        }
    };
    file.flush()?;
    drop(file);
    if let Err(error) = std::fs::rename(&temp_path, output_file) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error)
            .with_context(|| format!("failed to move download into {}", output_file.display()));
    }
    Ok(written)
}

fn download_temp_path(output_file: &Path) -> std::path::PathBuf {
    let suffix = format!("tmp-{}-{}", std::process::id(), uuid::Uuid::new_v4());
    match output_file.file_name().and_then(|name| name.to_str()) {
        Some(name) => output_file.with_file_name(format!("{name}.{suffix}")),
        None => output_file.with_extension(suffix),
    }
}

fn read_response_headers(stream: &mut impl Read) -> Result<Vec<u8>> {
    let mut headers = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        let read = stream.read(&mut byte)?;
        anyhow::ensure!(read != 0, "invalid HTTP response from API");
        headers.push(byte[0]);
        anyhow::ensure!(
            headers.len() <= MAX_API_RESPONSE_HEADER_BYTES,
            "API response headers exceeded {MAX_API_RESPONSE_HEADER_BYTES} bytes"
        );
        if headers.ends_with(b"\r\n\r\n") {
            return Ok(headers);
        }
    }
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (candidate, value) = line.split_once(':')?;
        candidate.eq_ignore_ascii_case(name).then_some(value.trim())
    })
}

fn copy_exact_body_to_file(
    stream: &mut impl Read,
    file: &mut File,
    mut remaining: u64,
) -> Result<u64> {
    let mut written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let max_read = buffer.len().min(remaining as usize);
        let read = stream.read(&mut buffer[..max_read])?;
        anyhow::ensure!(read != 0, "API response ended before content-length");
        file.write_all(&buffer[..read])?;
        written = written.saturating_add(read as u64);
        remaining = remaining.saturating_sub(read as u64);
    }
    Ok(written)
}

fn copy_body_to_file(stream: &mut impl Read, file: &mut File) -> Result<u64> {
    let mut written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(written);
        }
        file.write_all(&buffer[..read])?;
        written = written.saturating_add(read as u64);
    }
}

fn copy_chunked_body_to_file(stream: &mut impl Read, file: &mut File) -> Result<u64> {
    let mut written = 0_u64;
    loop {
        let line = read_crlf_line(stream)?;
        let chunk_size_hex = line
            .split(';')
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("invalid chunked API response")?;
        let chunk_size =
            u64::from_str_radix(chunk_size_hex, 16).context("invalid API response chunk size")?;
        if chunk_size == 0 {
            loop {
                if read_crlf_line(stream)?.is_empty() {
                    return Ok(written);
                }
            }
        }
        written = written.saturating_add(copy_exact_body_to_file(stream, file, chunk_size)?);
        let mut crlf = [0_u8; 2];
        stream.read_exact(&mut crlf)?;
        anyhow::ensure!(crlf == *b"\r\n", "invalid chunked API response");
    }
}

fn read_crlf_line(stream: &mut impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        let read = stream.read(&mut byte)?;
        anyhow::ensure!(read != 0, "API response ended mid-line");
        bytes.push(byte[0]);
        anyhow::ensure!(
            bytes.len() <= MAX_API_RESPONSE_HEADER_BYTES,
            "API response line exceeded {MAX_API_RESPONSE_HEADER_BYTES} bytes"
        );
        if bytes.ends_with(b"\r\n") {
            bytes.truncate(bytes.len().saturating_sub(2));
            return String::from_utf8(bytes).context("API response line is not UTF-8");
        }
    }
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

    #[test]
    fn streams_content_length_response_to_file() {
        let mut stream =
            FakeHttpStream::new(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world");
        let path = temp_download_test_path("content-length");
        let written = write_request_and_stream_response_to_file(
            &mut stream,
            "GET /artifact HTTP/1.1\r\n\r\n",
            "GET",
            "/artifact",
            &path,
        )
        .unwrap();

        assert_eq!(written, 11);
        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn streams_chunked_response_to_file() {
        let mut stream =
            FakeHttpStream::new(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
        let path = temp_download_test_path("chunked");
        let written = write_request_and_stream_response_to_file(
            &mut stream,
            "GET /artifact HTTP/1.1\r\n\r\n",
            "GET",
            "/artifact",
            &path,
        )
        .unwrap();

        assert_eq!(written, 11);
        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
        let _ = std::fs::remove_file(path);
    }

    struct FakeHttpStream {
        read: std::io::Cursor<Vec<u8>>,
        written: Vec<u8>,
    }

    impl FakeHttpStream {
        fn new(response: &[u8]) -> Self {
            Self {
                read: std::io::Cursor::new(response.to_vec()),
                written: Vec::new(),
            }
        }
    }

    impl std::io::Read for FakeHttpStream {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            std::io::Read::read(&mut self.read, buffer)
        }
    }

    impl std::io::Write for FakeHttpStream {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn temp_download_test_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "vpsctl-http-download-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }
}
