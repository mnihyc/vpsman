use super::*;

const TEST_BUDGET: Duration = Duration::from_secs(5);

fn loopback_plan() -> TunnelPlan {
    let mut plan = tests::speed_test_plan();
    plan.left_tunnel_address = "127.0.0.2".to_string();
    plan.right_tunnel_address = "127.0.0.1".to_string();
    let pair = plan.ipv4_tunnel.as_mut().unwrap();
    pair.left = plan.left_tunnel_address.clone();
    pair.right = plan.right_tunnel_address.clone();
    pair.prefix_len = 8;
    plan.tunnel_prefix_len = 8;
    plan
}

fn unused_loopback_port() -> u16 {
    // Production owns its listener; reserve an ephemeral port immediately before
    // polling it, and let the real client retry until that listener is ready.
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn role_input<'a>(
    plan: &'a TunnelPlan,
    port: u16,
    role: &'static str,
) -> NetworkSpeedRoleInput<'a> {
    NetworkSpeedRoleInput {
        job_id: uuid::Uuid::nil(),
        command_payload_hash: "transport-regression",
        plan,
        client_id: if role == "server" {
            "right-vps"
        } else {
            "left-vps"
        },
        peer_client_id: if role == "server" {
            "left-vps"
        } else {
            "right-vps"
        },
        role,
        server_side: TunnelEndpointSide::Right,
        server_address: &plan.right_tunnel_address,
        peer_tunnel_address: &plan.left_tunnel_address,
        port,
        duration: Duration::from_secs(2),
        max_bytes: NETWORK_SPEED_TEST_UNLIMITED_MAX_BYTES,
        rate_limit_kbps: NETWORK_SPEED_TEST_UNLIMITED_RATE_LIMIT_KBPS,
        connect_timeout: Duration::from_secs(2),
    }
}

async fn authenticated_writer(port: u16) -> TcpStream {
    let mut stream = connect_with_retry(
        SocketAddr::from(([127, 0, 0, 1], port)),
        SocketAddr::from(([127, 0, 0, 2], 0)),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let nonce = speed_test_nonce_hex(uuid::Uuid::nil(), "transport-regression");
    write_speed_test_handshake(&mut stream, &nonce)
        .await
        .unwrap();
    assert!(read_speed_test_ack(&mut stream, Duration::from_secs(2))
        .await
        .unwrap());
    stream
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    time::timeout(TEST_BUDGET, future)
        .await
        .expect("transport test stalled")
}

fn measured_status(output: CommandOutput, bytes: u64, success: bool) -> serde_json::Value {
    assert_eq!(output.exit_code, Some(if success { 0 } else { 1 }));
    let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();
    assert_eq!(status["success"], success);
    assert_eq!(status["bytes"], bytes);
    let elapsed_ms = status["elapsed_ms"].as_f64().unwrap();
    let throughput = status["throughput_mbps"].as_f64().unwrap();
    assert!(elapsed_ms > 0.0 && throughput > 0.0);
    assert!((throughput - bytes as f64 * 8.0 / elapsed_ms / 1_000.0).abs() < 1e-9);
    let intervals = status["throughput_intervals"].as_array().unwrap();
    assert_eq!(
        intervals
            .iter()
            .map(|v| v["bytes"].as_u64().unwrap())
            .sum::<u64>(),
        bytes
    );
    assert!(intervals
        .iter()
        .all(|v| v["end_ms"].as_f64().unwrap() <= elapsed_ms));
    status
}

#[tokio::test]
async fn duration_boundary_keeps_receiver_open_and_excludes_tail_from_measurement() {
    let plan = loopback_plan();
    let port = unused_loopback_port();
    let mut input = role_input(&plan, port, "server");
    input.duration = Duration::from_millis(200);
    let writer = async {
        let mut stream = authenticated_writer(port).await;
        let started = Instant::now();
        stream.write_all(&[1; SPEED_CHUNK_BYTES]).await.unwrap();
        // Three measurement windows leave scheduling slack while proving the
        // receiver has not closed at its own deadline. No response is expected.
        assert!(
            time::timeout(Duration::from_millis(600), stream.read(&mut [0]))
                .await
                .is_err()
        );
        let tail_started_ms = started.elapsed().as_secs_f64() * 1_000.0;
        stream.write_all(&[2; SPEED_CHUNK_BYTES]).await.unwrap();
        stream.shutdown().await.unwrap();
        tail_started_ms
    };
    let (output, tail_started_ms) =
        bounded(async { tokio::join!(receive_speed_test(input), writer) }).await;
    let status = measured_status(output.unwrap(), SPEED_CHUNK_BYTES as u64, true);
    assert!(status["elapsed_ms"].as_f64().unwrap() >= 200.0);
    assert!(status["elapsed_ms"].as_f64().unwrap() < tail_started_ms);
}

#[tokio::test]
async fn byte_cap_boundary_waits_for_eof_and_discards_extra_payload() {
    let plan = loopback_plan();
    let port = unused_loopback_port();
    let mut input = role_input(&plan, port, "server");
    input.max_bytes = SPEED_CHUNK_BYTES as u64;
    let writer = async {
        let mut stream = authenticated_writer(port).await;
        let started = Instant::now();
        // One write crosses the exact cap independently of packet boundaries.
        stream.write_all(&[1; SPEED_CHUNK_BYTES * 2]).await.unwrap();
        assert!(
            time::timeout(Duration::from_millis(200), stream.read(&mut [0]))
                .await
                .is_err()
        );
        let before_eof_ms = started.elapsed().as_secs_f64() * 1_000.0;
        stream.shutdown().await.unwrap();
        before_eof_ms
    };
    let (output, before_eof_ms) =
        bounded(async { tokio::join!(receive_speed_test(input), writer) }).await;
    let status = measured_status(output.unwrap(), SPEED_CHUNK_BYTES as u64, true);
    assert!(status["elapsed_ms"].as_f64().unwrap() < before_eof_ms);
}

#[tokio::test]
async fn real_sender_and_receiver_complete_byte_capped_and_rate_limited_transfers() {
    for rate_limit_kbps in [0, 1_024] {
        let plan = loopback_plan();
        let port = unused_loopback_port();
        let mut receiver = role_input(&plan, port, "server");
        let mut sender = role_input(&plan, port, "client");
        // Two chunks exercise pacing between writes without a long-running test.
        let cap = SPEED_CHUNK_BYTES as u64 * 2;
        receiver.max_bytes = cap;
        sender.max_bytes = cap;
        receiver.rate_limit_kbps = rate_limit_kbps;
        sender.rate_limit_kbps = rate_limit_kbps;
        let (received, sent) =
            bounded(async { tokio::join!(receive_speed_test(receiver), send_speed_test(sender)) })
                .await;
        measured_status(received.unwrap(), cap, true);
        measured_status(sent.unwrap(), cap, true);
    }
}

#[tokio::test]
async fn early_sender_eof_still_reports_an_incomplete_transfer() {
    let plan = loopback_plan();
    let port = unused_loopback_port();
    let writer = async {
        let mut stream = authenticated_writer(port).await;
        stream.write_all(&[1; SPEED_CHUNK_BYTES]).await.unwrap();
        stream.shutdown().await.unwrap();
    };
    let (output, ()) = bounded(async {
        tokio::join!(
            receive_speed_test(role_input(&plan, port, "server")),
            writer
        )
    })
    .await;
    let status = measured_status(output.unwrap(), SPEED_CHUNK_BYTES as u64, false);
    assert_eq!(status["reason"], "transfer_incomplete");
}

#[tokio::test]
async fn peer_reset_during_drain_remains_a_transport_error() {
    let plan = loopback_plan();
    let port = unused_loopback_port();
    let mut input = role_input(&plan, port, "server");
    input.max_bytes = SPEED_CHUNK_BYTES as u64;
    let writer = async {
        let mut stream = authenticated_writer(port).await;
        stream.write_all(&[1; SPEED_CHUNK_BYTES]).await.unwrap();
        assert!(
            time::timeout(Duration::from_millis(100), stream.read(&mut [0]))
                .await
                .is_err()
        );
        // Abort this actual TCP connection, rather than synthesizing an I/O error.
        #[allow(deprecated)]
        stream.set_linger(Some(Duration::ZERO)).unwrap();
    };
    let (result, ()) = bounded(async { tokio::join!(receive_speed_test(input), writer) }).await;
    let error = result.unwrap_err();
    assert!(error
        .to_string()
        .contains("failed to drain speed-test stream"));
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::ConnectionReset
    );
}

async fn stalled_drain_error(cancel: bool) -> anyhow::Error {
    let plan = loopback_plan();
    let port = unused_loopback_port();
    let config = AgentConfig {
        client_id: "right-vps".to_string(),
        ..AgentConfig::default()
    };
    let token = CommandCancelToken::default();
    let command = execute_network_speed_test_command(NetworkSpeedTestInput {
        job_id: uuid::Uuid::nil(),
        command_payload_hash: "transport-regression",
        config: &config,
        plan: &plan,
        server_side: TunnelEndpointSide::Right,
        duration_secs: 2,
        max_bytes: SPEED_CHUNK_BYTES as u64,
        rate_limit_kbps: 0,
        port,
        connect_timeout_ms: 1_000,
        max_timeout_secs: if cancel { 4 } else { 1 },
        cancel_token: token.clone(),
    });
    let writer = async {
        let mut stream = authenticated_writer(port).await;
        stream.write_all(&[1; SPEED_CHUNK_BYTES]).await.unwrap();
        assert!(
            time::timeout(Duration::from_millis(100), stream.read(&mut [0]))
                .await
                .is_err()
        );
        if cancel {
            token.cancel("cancel stalled drain".to_string());
        }
        // Keep the write half open until production timeout/cancellation drops
        // the receiver. The test watchdog bounds a missing cancellation path.
        assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
    };
    let (result, ()) = bounded(async { tokio::join!(command, writer) }).await;
    result.unwrap_err()
}

#[tokio::test]
async fn stalled_drain_is_bounded_by_the_existing_command_timeout() {
    assert!(stalled_drain_error(false)
        .await
        .to_string()
        .contains("network speed test timed out"));
}

#[tokio::test]
async fn stalled_drain_obeys_existing_command_cancellation() {
    let error = stalled_drain_error(true).await;
    let canceled = error
        .downcast_ref::<crate::command_worker::CommandCanceled>()
        .unwrap();
    assert_eq!(canceled.operation_type(), "network_speed_test");
    assert_eq!(canceled.reason(), "cancel stalled drain");
}
