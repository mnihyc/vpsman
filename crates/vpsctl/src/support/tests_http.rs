use super::*;
use std::os::unix::fs::PermissionsExt;

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
    assert!(message.contains("operator scope or privilege assertion"));

    let conflict = b"HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: 88\r\n\r\n{\"error\":\"confirmation_snapshot_stale\",\"message\":\"Target set changed after review\",\"status\":409}";
    let message = decode_api_response("POST", "/api/v1/jobs", conflict)
        .unwrap_err()
        .to_string();
    assert!(message.contains("Confirmation snapshot stale"));
    assert!(message.contains("Target set changed after review"));
    assert!(message.contains("refresh it and review the action again"));
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
    assert_eq!(mode(&path), 0o600);
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
    assert_eq!(mode(&path), 0o600);
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

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}
