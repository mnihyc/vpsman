use super::*;
use tokio::io::duplex;

#[tokio::test]
async fn preserves_gateway_control_error_status() {
    let (mut client, mut server) = duplex(4_096);
    let server_task = tokio::spawn(async move {
        let mut request = vec![0_u8; 4_096];
        let _ = server.read(&mut request).await.unwrap();
        server
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: 32\r\nConnection: close\r\n\r\n{\"error\":\"verifier unavailable\"}",
            )
            .await
            .unwrap();
    });

    let error = send_gateway_control_request::<_, serde_json::Value>(
        &mut client,
        "gateway-control",
        "/internal/v1/gateway/privilege/verify",
        b"{}",
        "internal-token",
        GatewayClientTimeouts::default(),
    )
    .await
    .unwrap_err();
    server_task.await.unwrap();

    let response = error
        .downcast_ref::<GatewayControlResponseError>()
        .expect("structured gateway response error");
    assert_eq!(response.status_code, 503);
    assert!(response.response_body.contains("verifier unavailable"));
}

#[tokio::test]
async fn dispatch_fence_batches_use_one_ordered_request_per_phase() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let expected_paths = [
            "/internal/v1/gateway/client/dispatch-fence/batch/prepare",
            "/internal/v1/gateway/client/dispatch-fence/batch/promote",
            "/internal/v1/gateway/client/dispatch-fence/batch/clear",
        ];
        for expected_path in expected_paths {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (path, request_body) = read_test_http_request(&mut stream).await;
            assert_eq!(path, expected_path);
            assert_eq!(request_body["items"][0]["client_id"], "client-b");
            assert_eq!(request_body["items"][1]["client_id"], "client-a");
            let fenced = !expected_path.ends_with("/clear");
            let response_body = serde_json::to_vec(&serde_json::json!({
                "results": [
                    {
                        "client_id": "client-b",
                        "accepted": true,
                        "fenced": fenced,
                        "ownership_continuous": true,
                        "enqueued_job_ids": [],
                        "message": "accepted"
                    },
                    {
                        "client_id": "client-a",
                        "accepted": true,
                        "fenced": fenced,
                        "ownership_continuous": true,
                        "enqueued_job_ids": [],
                        "message": "accepted"
                    }
                ]
            }))
            .unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        response_body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&response_body).await.unwrap();
        }
    });

    let client = GatewayDispatchClient::new_with_timeouts(
        Some(format!("http://{address}")),
        Some("internal-token".to_string()),
        GatewayClientTimeouts::default(),
    );
    let client_b_token = uuid::Uuid::new_v4();
    let client_a_token = uuid::Uuid::new_v4();
    let gateway_epoch = uuid::Uuid::new_v4();
    let prepared = client
        .prepare_client_dispatch_fences(vec![
            GatewayClientDispatchFencePrepare {
                client_id: "client-b".to_string(),
                token: client_b_token,
                gateway_epoch,
                generation: 1,
                renewal: false,
                lease_secs: 60,
                purpose: GatewayClientDispatchFencePurpose::Suspension,
            },
            GatewayClientDispatchFencePrepare {
                client_id: "client-a".to_string(),
                token: client_a_token,
                gateway_epoch,
                generation: 2,
                renewal: false,
                lease_secs: 60,
                purpose: GatewayClientDispatchFencePurpose::Deletion,
            },
        ])
        .await
        .unwrap();
    assert_eq!(prepared.results[0].client_id, "client-b");
    assert_eq!(prepared.results[1].client_id, "client-a");

    let promoted = client
        .promote_client_dispatch_fences(vec![
            GatewayClientDispatchFencePromote {
                client_id: "client-b".to_string(),
                token: client_b_token,
                gateway_epoch,
                generation: 1,
                purpose: GatewayClientDispatchFencePurpose::Suspension,
            },
            GatewayClientDispatchFencePromote {
                client_id: "client-a".to_string(),
                token: client_a_token,
                gateway_epoch,
                generation: 2,
                purpose: GatewayClientDispatchFencePurpose::Deletion,
            },
        ])
        .await
        .unwrap();
    assert_eq!(promoted.results[0].client_id, "client-b");
    assert_eq!(promoted.results[1].client_id, "client-a");

    let cleared = client
        .clear_client_dispatch_fences(vec![
            GatewayClientDispatchFenceClear {
                client_id: "client-b".to_string(),
                expected_token: client_b_token,
                gateway_epoch,
                expected_generation: 1,
                restore_fallback: false,
                reason: "committed_unsuspend".to_string(),
            },
            GatewayClientDispatchFenceClear {
                client_id: "client-a".to_string(),
                expected_token: client_a_token,
                gateway_epoch,
                expected_generation: 2,
                restore_fallback: false,
                reason: "committed_unsuspend".to_string(),
            },
        ])
        .await
        .unwrap();
    assert_eq!(cleared.results[0].client_id, "client-b");
    assert_eq!(cleared.results[1].client_id, "client-a");
    server.await.unwrap();
}

#[tokio::test]
async fn lifecycle_batches_use_one_ordered_request_per_operation() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (path, request_body) = read_test_http_request(&mut stream).await;
        assert_eq!(path, "/internal/v1/gateway/privilege/verify/batch");
        assert_eq!(request_body["items"][0]["request_id"], "client-b");
        assert_eq!(request_body["items"][1]["request_id"], "client-a");
        let response_body = serde_json::to_vec(&serde_json::json!({
            "results": [
                {
                    "request_id": "client-b",
                    "approved": true,
                    "intent_hash_hex": "hash-b",
                    "message": "approved",
                    "error_code": null
                },
                {
                    "request_id": "client-a",
                    "approved": false,
                    "intent_hash_hex": null,
                    "message": "rejected",
                    "error_code": "privilege_assertion_Replay"
                }
            ]
        }))
        .unwrap();
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response_body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(&response_body).await.unwrap();
        drop(stream);

        let (mut stream, _) = listener.accept().await.unwrap();
        let (path, request_body) = read_test_http_request(&mut stream).await;
        assert_eq!(path, "/internal/v1/gateway/session/disconnect/batch");
        assert_eq!(request_body["items"][0]["client_id"], "client-b");
        assert_eq!(request_body["items"][1]["client_id"], "client-a");
        let response_body = serde_json::to_vec(&serde_json::json!({
            "results": [
                {
                    "client_id": "client-b",
                    "accepted": true,
                    "disconnected": true,
                    "message": "disconnect_requested"
                },
                {
                    "client_id": "client-a",
                    "accepted": true,
                    "disconnected": false,
                    "message": "agent_not_online"
                }
            ]
        }))
        .unwrap();
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response_body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(&response_body).await.unwrap();
    });

    let client = GatewayDispatchClient::new_with_timeouts(
        Some(format!("http://{address}")),
        Some("internal-token".to_string()),
        GatewayClientTimeouts::default(),
    );
    let assertion = PrivilegeAssertion {
        nonce_hex: "00".repeat(16),
        issued_unix: 1,
        expires_unix: 2,
        assertion_hex: "00".repeat(32),
    };
    let privilege = client
        .verify_privileges(vec![
            GatewayPrivilegeVerificationBatchItem {
                request_id: "client-b".to_string(),
                verification: GatewayPrivilegeVerification {
                    intent: "intent-b".to_string(),
                    assertion: assertion.clone(),
                },
            },
            GatewayPrivilegeVerificationBatchItem {
                request_id: "client-a".to_string(),
                verification: GatewayPrivilegeVerification {
                    intent: "intent-a".to_string(),
                    assertion,
                },
            },
        ])
        .await
        .unwrap();
    assert_eq!(privilege.results[0].request_id, "client-b");
    assert!(privilege.results[0].approved);
    assert_eq!(privilege.results[1].request_id, "client-a");
    assert!(!privilege.results[1].approved);

    let disconnected = client
        .disconnect_sessions(vec![
            GatewaySessionDisconnect {
                client_id: "client-b".to_string(),
                reason: "vps_deleted".to_string(),
                required_dispatch_fence_owner: None,
            },
            GatewaySessionDisconnect {
                client_id: "client-a".to_string(),
                reason: "vps_deleted".to_string(),
                required_dispatch_fence_owner: None,
            },
        ])
        .await
        .unwrap();
    assert_eq!(disconnected.results[0].client_id, "client-b");
    assert!(disconnected.results[0].disconnected);
    assert_eq!(disconnected.results[1].client_id, "client-a");
    assert!(!disconnected.results[1].disconnected);
    server.await.unwrap();
}

async fn read_test_http_request(stream: &mut tokio::net::TcpStream) -> (String, serde_json::Value) {
    let mut request = Vec::new();
    let (header_end, content_length) = loop {
        let mut chunk = [0_u8; 4_096];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "gateway test request closed before headers");
        request.extend_from_slice(&chunk[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&request[..header_end]).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("Content-Length: ")
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap();
        break (header_end, content_length);
    };
    let body_start = header_end + 4;
    while request.len() < body_start + content_length {
        let mut chunk = [0_u8; 4_096];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "gateway test request closed before its body");
        request.extend_from_slice(&chunk[..read]);
    }
    let headers = std::str::from_utf8(&request[..header_end]).unwrap();
    let path = headers
        .lines()
        .next()
        .unwrap()
        .split_ascii_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    let body = serde_json::from_slice(&request[body_start..body_start + content_length]).unwrap();
    (path, body)
}
