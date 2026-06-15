use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::object_store::{validate_object_key, BackupObjectStore, S3BackupObjectStoreSettings};

#[tokio::test]
async fn filesystem_object_store_writes_under_safe_relative_key() {
    let root = std::env::temp_dir().join(format!("vpsman-object-store-{}", Uuid::new_v4()));
    let store = BackupObjectStore::filesystem(root.clone()).unwrap();
    store
        .put_new("backups/client-a/artifact.json", b"ciphertext")
        .await
        .unwrap();
    assert_eq!(
        store.get("backups/client-a/artifact.json").await.unwrap(),
        b"ciphertext"
    );
    let path = root.join("backups/client-a/artifact.json");

    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"ciphertext");
    store
        .delete_best_effort("backups/client-a/artifact.json")
        .await;
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn filesystem_verified_object_file_returns_store_path_without_cleanup() {
    let root = std::env::temp_dir().join(format!("vpsman-object-store-{}", Uuid::new_v4()));
    let store = BackupObjectStore::filesystem(root.clone()).unwrap();
    let payload = b"verified filesystem payload";
    let object_key = "backups/client-a/verified.bin";
    store.put_new(object_key, payload).await.unwrap();

    let verified = store
        .verified_object_file(object_key, &sha256_hex(payload), payload.len() as u64, 1024)
        .await
        .unwrap();

    assert!(!verified.cleanup_after_stream);
    assert_eq!(tokio::fs::read(&verified.path).await.unwrap(), payload);
    assert!(verified.path.starts_with(&root));
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn filesystem_confirmed_delete_removes_object_and_accepts_missing() {
    let root = std::env::temp_dir().join(format!("vpsman-object-store-{}", Uuid::new_v4()));
    let store = BackupObjectStore::filesystem(root.clone()).unwrap();
    let object_key = "jobs/client-a/output.bin";
    store.put_new(object_key, b"output").await.unwrap();

    store.delete_confirmed(object_key).await.unwrap();
    store.delete_confirmed(object_key).await.unwrap();
    assert!(store.get(object_key).await.is_err());
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[test]
fn object_key_rejects_path_traversal() {
    assert!(validate_object_key("../artifact").is_err());
    assert!(validate_object_key("backups//artifact").is_err());
    assert!(validate_object_key("/backups/artifact").is_err());
    assert!(validate_object_key("backups\\artifact").is_err());
}

#[test]
fn s3_settings_accept_https_and_reject_unsafe_bucket() {
    assert!(BackupObjectStore::s3(S3BackupObjectStoreSettings {
        endpoint: "https://s3.example".to_string(),
        bucket: "vpsman-artifacts".to_string(),
        access_key: "access".to_string(),
        secret_key: "secret".to_string(),
        region: "us-east-1".to_string(),
        create_bucket: false,
    })
    .is_ok());
    assert!(BackupObjectStore::s3(S3BackupObjectStoreSettings {
        endpoint: "http://127.0.0.1:9000".to_string(),
        bucket: "../bad".to_string(),
        access_key: "access".to_string(),
        secret_key: "secret".to_string(),
        region: "us-east-1".to_string(),
        create_bucket: false,
    })
    .is_err());
}

#[tokio::test]
async fn s3_object_store_put_get_delete_uses_signed_path_style_requests() {
    let object_key = "backups/client-a/artifact.bin";
    let object_path = "/root/vpsman-artifacts/backups/client-a/artifact.bin";
    let payload = b"object-store payload".to_vec();
    let (endpoint, server) = spawn_fake_s3(vec![
        ExpectedS3Request::new("HEAD", object_path, Vec::new(), s3_response(404, b"")),
        ExpectedS3Request::new("PUT", object_path, payload.clone(), s3_response(200, b"")),
        ExpectedS3Request::new("GET", object_path, Vec::new(), s3_response(200, &payload)),
        ExpectedS3Request::new("DELETE", object_path, Vec::new(), s3_response(204, b"")),
    ])
    .await;
    let store = s3_store(&endpoint);

    store.put_new(object_key, &payload).await.unwrap();
    assert_eq!(store.get(object_key).await.unwrap(), payload);
    store.delete_best_effort(object_key).await;
    server.await.unwrap();
}

#[tokio::test]
async fn s3_confirmed_delete_accepts_not_found_and_rejects_server_error() {
    let object_key = "backups/client-a/delete.bin";
    let object_path = "/root/vpsman-artifacts/backups/client-a/delete.bin";
    let (endpoint, server) = spawn_fake_s3(vec![
        ExpectedS3Request::new("DELETE", object_path, Vec::new(), s3_response(404, b"")),
        ExpectedS3Request::new("DELETE", object_path, Vec::new(), s3_response(500, b"boom")),
    ])
    .await;
    let store = s3_store(&endpoint);

    store.delete_confirmed(object_key).await.unwrap();
    let error = store
        .delete_confirmed(object_key)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("S3 delete object failed with HTTP 500"));
    server.await.unwrap();
}

#[tokio::test]
async fn s3_object_store_duplicate_head_response_does_not_wait_for_body() {
    let object_key = "backups/client-a/existing.bin";
    let object_path = "/root/vpsman-artifacts/backups/client-a/existing.bin";
    let payload = b"new payload".to_vec();
    let (endpoint, server) = spawn_fake_s3(vec![ExpectedS3Request::new(
        "HEAD",
        object_path,
        Vec::new(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 8192\r\n\r\n".to_vec(),
    )])
    .await;
    let store = s3_store(&endpoint);

    let error = store
        .put_new(object_key, &payload)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("object already exists"));
    server.await.unwrap();
}

#[tokio::test]
async fn s3_object_store_decodes_chunked_get_response() {
    let object_key = "backups/client-a/chunked.bin";
    let object_path = "/root/vpsman-artifacts/backups/client-a/chunked.bin";
    let (endpoint, server) = spawn_fake_s3(vec![ExpectedS3Request::new(
        "GET",
        object_path,
        Vec::new(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"
            .to_vec(),
    )])
    .await;
    let store = s3_store(&endpoint);

    assert_eq!(store.get(object_key).await.unwrap(), b"hello world");
    server.await.unwrap();
}

#[tokio::test]
async fn s3_verified_object_file_spools_hash_checked_temp_file() {
    let object_key = "backups/client-a/spooled.bin";
    let object_path = "/root/vpsman-artifacts/backups/client-a/spooled.bin";
    let payload = b"spooled object-store payload".to_vec();
    let (endpoint, server) = spawn_fake_s3(vec![ExpectedS3Request::new(
        "GET",
        object_path,
        Vec::new(),
        s3_response(200, &payload),
    )])
    .await;
    let store = s3_store(&endpoint);

    let verified = store
        .verified_object_file(
            object_key,
            &sha256_hex(&payload),
            payload.len() as u64,
            payload.len(),
        )
        .await
        .unwrap();

    assert!(verified.cleanup_after_stream);
    assert_eq!(tokio::fs::read(&verified.path).await.unwrap(), payload);
    tokio::fs::remove_file(&verified.path).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn s3_object_store_rejects_truncated_content_length() {
    let object_key = "backups/client-a/truncated.bin";
    let object_path = "/root/vpsman-artifacts/backups/client-a/truncated.bin";
    let (endpoint, server) = spawn_fake_s3(vec![ExpectedS3Request::new(
        "GET",
        object_path,
        Vec::new(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort".to_vec(),
    )])
    .await;
    let store = s3_store(&endpoint);

    let error = store.get(object_key).await.unwrap_err().to_string();
    assert!(error.contains("failed to read S3 response"));
    server.await.unwrap();
}

fn s3_store(endpoint: &str) -> BackupObjectStore {
    BackupObjectStore::s3(S3BackupObjectStoreSettings {
        endpoint: endpoint.to_string(),
        bucket: "vpsman-artifacts".to_string(),
        access_key: "access".to_string(),
        secret_key: "secret".to_string(),
        region: "us-east-1".to_string(),
        create_bucket: false,
    })
    .unwrap()
}

async fn spawn_fake_s3(expected: Vec<ExpectedS3Request>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/root", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for expectation in expected {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut socket).await;
            assert_eq!(request.method, expectation.method);
            assert_eq!(request.path, expectation.path);
            assert_eq!(request.body, expectation.body);
            assert_header_prefix(&request.headers, "authorization", "AWS4-HMAC-SHA256 ");
            let expected_hash = sha256_hex(&expectation.body);
            assert_eq!(
                request
                    .headers
                    .get("x-amz-content-sha256")
                    .map(String::as_str),
                Some(expected_hash.as_str())
            );
            socket.write_all(&expectation.response).await.unwrap();
        }
    });
    (endpoint, server)
}

async fn read_http_request(socket: &mut TcpStream) -> HttpRequest {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 8192];
        let read = socket.read(&mut chunk).await.unwrap();
        assert_ne!(read, 0, "request ended before headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = find_bytes(&bytes, b"\r\n\r\n") {
            break header_end;
        }
        assert!(bytes.len() <= 64 * 1024, "request headers too large");
    };
    let head = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap().to_string();
    let path = request_parts.next().unwrap().to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').unwrap();
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let mut body = bytes[header_end + 4..].to_vec();
    while body.len() < content_length {
        let mut chunk = [0_u8; 8192];
        let read = socket.read(&mut chunk).await.unwrap();
        assert_ne!(read, 0, "request ended before declared content-length");
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    HttpRequest {
        method,
        path,
        headers,
        body,
    }
}

fn assert_header_prefix(headers: &BTreeMap<String, String>, name: &str, prefix: &str) {
    let value = headers
        .get(name)
        .unwrap_or_else(|| panic!("missing {name} header"));
    assert!(
        value.starts_with(prefix),
        "header {name} value {value:?} did not start with {prefix:?}"
    );
}

fn s3_response(status: u16, body: &[u8]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        404 => "Not Found",
        _ => "Status",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

struct ExpectedS3Request {
    method: &'static str,
    path: &'static str,
    body: Vec<u8>,
    response: Vec<u8>,
}

impl ExpectedS3Request {
    fn new(method: &'static str, path: &'static str, body: Vec<u8>, response: Vec<u8>) -> Self {
        Self {
            method,
            path,
            body,
            response,
        }
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
