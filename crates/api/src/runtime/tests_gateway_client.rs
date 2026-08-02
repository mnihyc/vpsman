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
