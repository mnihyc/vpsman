use super::*;
use ed25519_dalek::SigningKey;
use std::{net::TcpListener, thread};
use vpsman_common::sign_discovery_document;

fn endpoint(label: &str, tcp_addr: &str, priority: u16) -> ServerEndpoint {
    ServerEndpoint {
        label: label.to_string(),
        tcp_addr: tcp_addr.to_string(),
        priority,
    }
}

#[test]
fn endpoint_candidates_sort_and_dedupe_config_and_discovery() {
    let config = AgentConfig {
        tcp_endpoints: vec![
            endpoint("primary", "10.0.0.1:9443", 20),
            endpoint("old", "10.0.0.2:9443", 30),
        ],
        ..AgentConfig::default()
    };
    let discovered = vec![
        endpoint("new-primary", "10.0.0.3:9443", 5),
        endpoint("duplicate", "10.0.0.1:9443", 1),
    ];

    let candidates = endpoint_candidates(&config, &discovered);

    assert_eq!(
        candidates
            .iter()
            .map(|endpoint| endpoint.tcp_addr.as_str())
            .collect::<Vec<_>>(),
        vec!["10.0.0.1:9443", "10.0.0.3:9443", "10.0.0.2:9443"]
    );
}

#[test]
fn validates_discovery_document_expiry_and_endpoint_shape() {
    let valid = DiscoveryDocument {
        version: 1,
        issued_unix: 100,
        expires_unix: 200,
        endpoints: vec![endpoint("primary", "198.51.100.10:9443", 10)],
        signature: Vec::new(),
    };

    assert_eq!(
        validate_discovery_document(valid, 150, &[], false).unwrap(),
        vec![endpoint("primary", "198.51.100.10:9443", 10)]
    );

    let expired = DiscoveryDocument {
        expires_unix: 149,
        ..DiscoveryDocument {
            version: 1,
            issued_unix: 100,
            expires_unix: 200,
            endpoints: vec![endpoint("primary", "198.51.100.10:9443", 10)],
            signature: Vec::new(),
        }
    };
    assert!(validate_discovery_document(expired, 150, &[], false)
        .unwrap_err()
        .to_string()
        .contains("expired"));

    let missing_port = DiscoveryDocument {
        endpoints: vec![endpoint("primary", "198.51.100.10", 10)],
        ..DiscoveryDocument {
            version: 1,
            issued_unix: 100,
            expires_unix: 200,
            endpoints: Vec::new(),
            signature: Vec::new(),
        }
    };
    assert!(validate_discovery_document(missing_port, 150, &[], false).is_err());
}

#[test]
fn verifies_signed_discovery_documents_when_key_is_configured() {
    let signing_key = SigningKey::from_bytes(&[17_u8; 32]);
    let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
    let mut document = DiscoveryDocument {
        version: 1,
        issued_unix: 100,
        expires_unix: 200,
        endpoints: vec![endpoint("primary", "198.51.100.10:9443", 10)],
        signature: Vec::new(),
    };
    document.signature = sign_discovery_document(&signing_key, &document);

    assert_eq!(
        validate_discovery_document(
            document.clone(),
            150,
            std::slice::from_ref(&public_key_hex),
            true,
        )
        .unwrap(),
        vec![endpoint("primary", "198.51.100.10:9443", 10)]
    );

    let mut tampered = document.clone();
    tampered.endpoints[0].tcp_addr = "203.0.113.20:9443".to_string();
    assert!(validate_discovery_document(
        tampered,
        150,
        std::slice::from_ref(&public_key_hex),
        true
    )
    .unwrap_err()
    .to_string()
    .contains("signature"));

    let mut unsigned = document;
    unsigned.signature.clear();
    assert!(
        validate_discovery_document(unsigned, 150, &[public_key_hex], true)
            .unwrap_err()
            .to_string()
            .contains("missing server signature")
    );
}

#[test]
fn verifies_discovery_documents_with_rotation_key_ring() {
    let current_signing_key = SigningKey::from_bytes(&[17_u8; 32]);
    let next_signing_key = SigningKey::from_bytes(&[18_u8; 32]);
    let current_public_key_hex = hex::encode(current_signing_key.verifying_key().to_bytes());
    let next_public_key_hex = hex::encode(next_signing_key.verifying_key().to_bytes());
    let mut document = DiscoveryDocument {
        version: 1,
        issued_unix: 100,
        expires_unix: 200,
        endpoints: vec![endpoint("primary", "198.51.100.10:9443", 10)],
        signature: Vec::new(),
    };
    document.signature = sign_discovery_document(&next_signing_key, &document);

    assert_eq!(
        validate_discovery_document(
            document,
            150,
            &[current_public_key_hex, next_public_key_hex],
            true
        )
        .unwrap(),
        vec![endpoint("primary", "198.51.100.10:9443", 10)]
    );
}

#[test]
fn requires_trusted_signature_for_https_discovery_policy() {
    let document = DiscoveryDocument {
        version: 1,
        issued_unix: 100,
        expires_unix: 200,
        endpoints: vec![endpoint("primary", "198.51.100.10:9443", 10)],
        signature: Vec::new(),
    };

    assert!(validate_discovery_document(document, 150, &[], true)
        .unwrap_err()
        .to_string()
        .contains("signature is required"));
}

#[test]
fn parses_https_and_rejects_nonlocal_http_discovery_urls() {
    assert_eq!(
        parse_discovery_url("https://panel.example/.well-known/vpsman/endpoints.json").unwrap(),
        ParsedDiscoveryUrl {
            scheme: DiscoveryScheme::Https,
            host: "panel.example".to_string(),
            port: 443,
            path_and_query: "/.well-known/vpsman/endpoints.json".to_string(),
        }
    );
    assert!(parse_discovery_url("http://panel.example/endpoints.json").is_err());
    assert!(parse_discovery_url("http://127.0.0.1:8080/endpoints.json").is_ok());
}

#[test]
fn decodes_simple_discovery_http_response() {
    let body = br#"{"version":1,"issued_unix":1,"expires_unix":2,"endpoints":[],"signature":[]}"#;
    let response = [
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ".as_slice(),
        body.len().to_string().as_bytes(),
        b"\r\n\r\n".as_slice(),
        body.as_slice(),
    ]
    .concat();

    assert_eq!(decode_http_response(&response).unwrap(), body);
}

#[test]
fn decodes_chunked_discovery_http_response() {
    let response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n{\"ok\":1\r\n1\r\n}\r\n0\r\n\r\n";

    assert_eq!(decode_http_response(response).unwrap(), br#"{"ok":1}"#);
}

#[tokio::test]
async fn refreshes_discovery_endpoints_from_local_dev_http() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let signing_key = SigningKey::from_bytes(&[19_u8; 32]);
    let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
    let expires_unix = unix_now().unwrap() + 60;
    let mut document = DiscoveryDocument {
        version: 1,
        issued_unix: 1,
        expires_unix,
        endpoints: vec![endpoint("fallback", "203.0.113.50:9443", 5)],
        signature: Vec::new(),
    };
    document.signature = sign_discovery_document(&signing_key, &document);
    let response_body = serde_json::to_string(&document).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 512];
        let _ = stream.read(&mut request).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    let config = AgentConfig {
        discovery_url: Some(format!("http://{addr}/endpoints.json")),
        auth: vpsman_common::AgentAuthConfig {
            server_ed25519_public_key_hex: Some(public_key_hex),
            ..Default::default()
        },
        ..AgentConfig::default()
    };

    let endpoints = refresh_discovery_endpoints(&config).await.unwrap();

    handle.join().unwrap();
    assert_eq!(
        endpoints,
        vec![endpoint("fallback", "203.0.113.50:9443", 5)]
    );
}
