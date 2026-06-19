use std::{
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpSocket, TcpStream},
    time,
};
use vpsman_common::{
    payload_hash, render_tunnel_endpoint_config, AgentConfig, CommandOutput, OutputStream,
    TunnelEndpointSide, TunnelPlan, NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS,
    NETWORK_SPEED_TEST_MAX_DURATION_SECS, NETWORK_SPEED_TEST_MAX_MAX_BYTES,
    NETWORK_SPEED_TEST_MAX_PORT, NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS,
    NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS, NETWORK_SPEED_TEST_MIN_DURATION_SECS,
    NETWORK_SPEED_TEST_MIN_MAX_BYTES, NETWORK_SPEED_TEST_MIN_PORT,
    NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS,
};

use crate::command_worker::{run_cancelable, CommandCancelToken};

const SPEED_CHUNK_BYTES: usize = 16 * 1024;
const CONNECT_RETRY_MS: u64 = 100;
const SPEED_HANDSHAKE_MAGIC: &[u8; 8] = b"VPSNST1\0";
const SPEED_HANDSHAKE_ACK: &[u8; 2] = b"OK";
const SPEED_HANDSHAKE_REJECT: &[u8; 2] = b"NO";
const SPEED_NONCE_HEX_BYTES: usize = 64;

pub(crate) struct NetworkSpeedTestInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) command_payload_hash: &'a str,
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) server_side: TunnelEndpointSide,
    pub(crate) duration_secs: u8,
    pub(crate) max_bytes: u64,
    pub(crate) rate_limit_kbps: u32,
    pub(crate) port: u16,
    pub(crate) connect_timeout_ms: u16,
    pub(crate) timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

pub(crate) async fn execute_network_speed_test_command(
    input: NetworkSpeedTestInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let cancel_token = input.cancel_token.clone();
    run_cancelable("network_speed_test", cancel_token, async move {
        time::timeout(
            Duration::from_secs(input.timeout_secs.max(1)),
            run_network_speed_test(input),
        )
        .await
        .context("network speed test timed out")?
    })
    .await
}

async fn run_network_speed_test(input: NetworkSpeedTestInput<'_>) -> Result<Vec<CommandOutput>> {
    validate_speed_test_budget(&input)?;
    let server_endpoint = render_tunnel_endpoint_config(input.plan, input.server_side)
        .map_err(|error| anyhow::anyhow!("invalid tunnel endpoint config: {error}"))?;
    let server_address = server_tunnel_address(input.plan, input.server_side);
    let duration = Duration::from_secs(u64::from(input.duration_secs.max(1)));
    let connect_timeout = Duration::from_millis(u64::from(input.connect_timeout_ms.max(100)));
    let result = if input.config.client_id == server_endpoint.local_client_id {
        receive_speed_test(NetworkSpeedRoleInput {
            job_id: input.job_id,
            command_payload_hash: input.command_payload_hash,
            plan: input.plan,
            client_id: &input.config.client_id,
            peer_client_id: &server_endpoint.peer_client_id,
            role: "server",
            server_side: input.server_side,
            server_address,
            peer_tunnel_address: &server_endpoint.remote_tunnel_address,
            port: input.port,
            duration,
            max_bytes: input.max_bytes,
            rate_limit_kbps: input.rate_limit_kbps,
            connect_timeout,
        })
        .await?
    } else if input.config.client_id == server_endpoint.peer_client_id {
        send_speed_test(NetworkSpeedRoleInput {
            job_id: input.job_id,
            command_payload_hash: input.command_payload_hash,
            plan: input.plan,
            client_id: &input.config.client_id,
            peer_client_id: &server_endpoint.local_client_id,
            role: "client",
            server_side: input.server_side,
            server_address,
            peer_tunnel_address: &server_endpoint.remote_tunnel_address,
            port: input.port,
            duration,
            max_bytes: input.max_bytes,
            rate_limit_kbps: input.rate_limit_kbps,
            connect_timeout,
        })
        .await?
    } else {
        anyhow::bail!(
            "network speed test targets endpoints {} and {}, but this agent is {}",
            server_endpoint.local_client_id,
            server_endpoint.peer_client_id,
            input.config.client_id
        );
    };
    Ok(vec![result])
}

fn validate_speed_test_budget(input: &NetworkSpeedTestInput<'_>) -> Result<()> {
    anyhow::ensure!(
        (NETWORK_SPEED_TEST_MIN_DURATION_SECS..=NETWORK_SPEED_TEST_MAX_DURATION_SECS)
            .contains(&input.duration_secs),
        "network speed test duration is out of range"
    );
    anyhow::ensure!(
        (NETWORK_SPEED_TEST_MIN_MAX_BYTES..=NETWORK_SPEED_TEST_MAX_MAX_BYTES)
            .contains(&input.max_bytes),
        "network speed test max bytes is out of range"
    );
    anyhow::ensure!(
        (NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS..=NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS)
            .contains(&input.rate_limit_kbps),
        "network speed test rate limit is out of range"
    );
    anyhow::ensure!(
        (NETWORK_SPEED_TEST_MIN_PORT..=NETWORK_SPEED_TEST_MAX_PORT).contains(&input.port),
        "network speed test port is out of range"
    );
    anyhow::ensure!(
        (NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS..=NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS)
            .contains(&input.connect_timeout_ms),
        "network speed test connect timeout is out of range"
    );
    Ok(())
}

struct NetworkSpeedRoleInput<'a> {
    job_id: uuid::Uuid,
    command_payload_hash: &'a str,
    plan: &'a TunnelPlan,
    client_id: &'a str,
    peer_client_id: &'a str,
    role: &'static str,
    server_side: TunnelEndpointSide,
    server_address: &'a str,
    peer_tunnel_address: &'a str,
    port: u16,
    duration: Duration,
    max_bytes: u64,
    rate_limit_kbps: u32,
    connect_timeout: Duration,
}

async fn receive_speed_test(input: NetworkSpeedRoleInput<'_>) -> Result<CommandOutput> {
    let bind_addr = socket_addr(input.server_address, input.port)?;
    let expected_peer_ip = ip_addr(input.peer_tunnel_address)?;
    let nonce_hex = speed_test_nonce_hex(input.job_id, input.command_payload_hash);
    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("failed to bind speed-test listener at {bind_addr}"))?;
    let started = Instant::now();
    let verification_deadline = started + input.connect_timeout;
    let mut last_peer_addr = None;
    let mut last_failure = None;
    let (mut stream, peer_addr) = loop {
        let remaining = verification_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let elapsed = started.elapsed();
            let failure = last_failure.unwrap_or(SpeedTestFailure {
                reason: "peer_verification_timeout",
                message: "network speed test timed out waiting for the expected peer",
            });
            return Ok(status_output(
                input,
                last_peer_addr,
                0,
                elapsed,
                false,
                Some(failure),
            ));
        }
        let (mut stream, peer_addr) = match time::timeout(remaining, listener.accept()).await {
            Ok(Ok(accepted)) => accepted,
            Ok(Err(error)) => return Err(error).context("failed to accept speed-test peer"),
            Err(_) => {
                let elapsed = started.elapsed();
                let failure = last_failure.unwrap_or(SpeedTestFailure {
                    reason: "peer_verification_timeout",
                    message: "network speed test timed out waiting for the expected peer",
                });
                return Ok(status_output(
                    input,
                    last_peer_addr,
                    0,
                    elapsed,
                    false,
                    Some(failure),
                ));
            }
        };
        last_peer_addr = Some(peer_addr);
        if peer_addr.ip() != expected_peer_ip {
            last_failure = Some(SpeedTestFailure {
                reason: "peer_address_mismatch",
                message: "network speed test rejected a connection from an unexpected peer address",
            });
            let _ = stream.write_all(SPEED_HANDSHAKE_REJECT).await;
            continue;
        }
        let handshake_remaining = verification_deadline.saturating_duration_since(Instant::now());
        if handshake_remaining.is_zero() {
            last_failure = Some(SpeedTestFailure {
                reason: "peer_verification_timeout",
                message: "network speed test timed out before peer handshake completed",
            });
            let _ = stream.write_all(SPEED_HANDSHAKE_REJECT).await;
            continue;
        }
        match read_speed_test_handshake(&mut stream, &nonce_hex, handshake_remaining).await {
            Ok(true) => {
                stream
                    .write_all(SPEED_HANDSHAKE_ACK)
                    .await
                    .context("failed to acknowledge speed-test peer")?;
                break (stream, peer_addr);
            }
            Ok(false) => {
                last_failure = Some(SpeedTestFailure {
                    reason: "peer_nonce_mismatch",
                    message: "network speed test rejected a peer with the wrong nonce",
                });
                let _ = stream.write_all(SPEED_HANDSHAKE_REJECT).await;
            }
            Err(_) => {
                last_failure = Some(SpeedTestFailure {
                    reason: "peer_handshake_failed",
                    message: "network speed test rejected a malformed or incomplete peer handshake",
                });
                let _ = stream.write_all(SPEED_HANDSHAKE_REJECT).await;
            }
        }
    };
    let mut buffer = vec![0_u8; SPEED_CHUNK_BYTES];
    let mut bytes_received = 0_u64;
    let deadline = Instant::now() + input.duration;
    while bytes_received < input.max_bytes && Instant::now() < deadline {
        let remaining = input.max_bytes - bytes_received;
        let read_limit =
            usize::try_from(remaining.min(SPEED_CHUNK_BYTES as u64)).unwrap_or(SPEED_CHUNK_BYTES);
        let read_timeout = deadline.saturating_duration_since(Instant::now());
        if read_timeout.is_zero() {
            break;
        }
        let bytes = match time::timeout(read_timeout, stream.read(&mut buffer[..read_limit])).await
        {
            Ok(Ok(0)) => break,
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error)) => return Err(error).context("failed to read speed-test stream"),
            Err(_) => break,
        };
        bytes_received = bytes_received.saturating_add(bytes as u64);
    }
    let elapsed = started.elapsed();
    Ok(status_output(
        input,
        Some(peer_addr),
        bytes_received,
        elapsed,
        bytes_received > 0,
        None,
    ))
}

async fn send_speed_test(input: NetworkSpeedRoleInput<'_>) -> Result<CommandOutput> {
    let target_addr = socket_addr(input.server_address, input.port)?;
    let local_addr = socket_addr(input.peer_tunnel_address, 0)?;
    let mut stream = connect_with_retry(target_addr, local_addr, input.connect_timeout).await?;
    let nonce_hex = speed_test_nonce_hex(input.job_id, input.command_payload_hash);
    write_speed_test_handshake(&mut stream, &nonce_hex)
        .await
        .context("failed to write speed-test peer handshake")?;
    match read_speed_test_ack(&mut stream, input.connect_timeout).await {
        Ok(true) => {}
        Ok(false) => {
            let elapsed = Duration::ZERO;
            return Ok(status_output(
                input,
                Some(target_addr),
                0,
                elapsed,
                false,
                Some(SpeedTestFailure {
                    reason: "peer_verification_rejected",
                    message: "network speed test server rejected the peer handshake",
                }),
            ));
        }
        Err(error) => {
            let message = error.to_string();
            let elapsed = Duration::ZERO;
            return Ok(status_output(
                input,
                Some(target_addr),
                0,
                elapsed,
                false,
                Some(SpeedTestFailure {
                    reason: "peer_verification_ack_failed",
                    message: &message,
                }),
            ));
        }
    }
    let started = Instant::now();
    let deadline = started + input.duration;
    let payload = vec![0_u8; SPEED_CHUNK_BYTES];
    let mut bytes_sent = 0_u64;
    while bytes_sent < input.max_bytes && Instant::now() < deadline {
        wait_for_rate_budget(started, bytes_sent, input.rate_limit_kbps).await;
        let remaining = input.max_bytes - bytes_sent;
        let write_limit =
            usize::try_from(remaining.min(SPEED_CHUNK_BYTES as u64)).unwrap_or(SPEED_CHUNK_BYTES);
        if write_limit == 0 {
            break;
        }
        match stream.write_all(&payload[..write_limit]).await {
            Ok(()) => bytes_sent = bytes_sent.saturating_add(write_limit as u64),
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => break,
            Err(error) => return Err(error).context("failed to write speed-test stream"),
        }
    }
    let _ = stream.shutdown().await;
    let elapsed = started.elapsed();
    Ok(status_output(
        input,
        Some(target_addr),
        bytes_sent,
        elapsed,
        bytes_sent > 0,
        None,
    ))
}

async fn connect_with_retry(
    target_addr: SocketAddr,
    local_addr: SocketAddr,
    timeout: Duration,
) -> Result<TcpStream> {
    let started = Instant::now();
    loop {
        match connect_from_local_addr(target_addr, local_addr).await {
            Ok(stream) => return Ok(stream),
            Err(error) if started.elapsed() < timeout => {
                let remaining = timeout.saturating_sub(started.elapsed());
                time::sleep(remaining.min(Duration::from_millis(CONNECT_RETRY_MS))).await;
                if remaining.is_zero() {
                    return Err(error).with_context(|| {
                        format!("failed to connect speed-test peer at {target_addr}")
                    });
                }
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to connect speed-test peer at {target_addr}")
                });
            }
        }
    }
}

async fn connect_from_local_addr(
    target_addr: SocketAddr,
    local_addr: SocketAddr,
) -> std::io::Result<TcpStream> {
    let socket = match target_addr {
        SocketAddr::V4(_) => TcpSocket::new_v4()?,
        SocketAddr::V6(_) => TcpSocket::new_v6()?,
    };
    socket.bind(local_addr)?;
    socket.connect(target_addr).await
}

async fn write_speed_test_handshake(stream: &mut TcpStream, nonce_hex: &str) -> Result<()> {
    stream.write_all(SPEED_HANDSHAKE_MAGIC).await?;
    stream.write_all(nonce_hex.as_bytes()).await?;
    Ok(())
}

async fn read_speed_test_handshake(
    stream: &mut TcpStream,
    expected_nonce_hex: &str,
    timeout: Duration,
) -> Result<bool> {
    let mut buffer = [0_u8; SPEED_HANDSHAKE_MAGIC.len() + SPEED_NONCE_HEX_BYTES];
    time::timeout(timeout, stream.read_exact(&mut buffer))
        .await
        .context("speed-test peer handshake timed out")?
        .context("failed to read speed-test peer handshake")?;
    Ok(
        &buffer[..SPEED_HANDSHAKE_MAGIC.len()] == SPEED_HANDSHAKE_MAGIC
            && &buffer[SPEED_HANDSHAKE_MAGIC.len()..] == expected_nonce_hex.as_bytes(),
    )
}

async fn read_speed_test_ack(stream: &mut TcpStream, timeout: Duration) -> Result<bool> {
    let mut ack = [0_u8; SPEED_HANDSHAKE_ACK.len()];
    time::timeout(timeout, stream.read_exact(&mut ack))
        .await
        .context("speed-test server acknowledgement timed out")?
        .context("failed to read speed-test server acknowledgement")?;
    Ok(&ack == SPEED_HANDSHAKE_ACK)
}

fn speed_test_nonce_hex(job_id: uuid::Uuid, command_payload_hash: &str) -> String {
    payload_hash(
        format!(
            "vpsman-network-speed-test-v1\n{job_id}\n{}\n",
            command_payload_hash.trim()
        )
        .as_bytes(),
    )
}

async fn wait_for_rate_budget(started: Instant, bytes_sent: u64, rate_limit_kbps: u32) {
    let bytes_per_sec = (u64::from(rate_limit_kbps).saturating_mul(1000) / 8).max(1);
    let allowed = started.elapsed().as_secs_f64() * bytes_per_sec as f64;
    if bytes_sent as f64 <= allowed {
        return;
    }
    let excess_bytes = bytes_sent as f64 - allowed;
    let wait_secs = excess_bytes / bytes_per_sec as f64;
    time::sleep(Duration::from_secs_f64(wait_secs.min(0.1))).await;
}

struct SpeedTestFailure<'a> {
    reason: &'a str,
    message: &'a str,
}

fn status_output(
    input: NetworkSpeedRoleInput<'_>,
    peer_addr: Option<SocketAddr>,
    bytes: u64,
    elapsed: Duration,
    success: bool,
    failure: Option<SpeedTestFailure<'_>>,
) -> CommandOutput {
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    let mbps = if elapsed.as_secs_f64() > 0.0 {
        (bytes as f64 * 8.0) / elapsed.as_secs_f64() / 1_000_000.0
    } else {
        0.0
    };
    let mut status = serde_json::json!({
        "type": "network_speed_test",
        "probe": "tcp_throughput",
        "role": input.role,
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "server_side": side_label(input.server_side),
        "client_id": input.client_id,
        "peer_client_id": input.peer_client_id,
        "server_address": input.server_address,
        "expected_peer_address": input.peer_tunnel_address,
        "port": input.port,
        "peer_socket": peer_addr.map(|addr| addr.to_string()),
        "duration_secs": input.duration.as_secs(),
        "max_bytes": input.max_bytes,
        "rate_limit_kbps": input.rate_limit_kbps,
        "bytes": bytes,
        "elapsed_ms": elapsed_ms,
        "throughput_mbps": mbps,
        "success": success,
    });
    if let Some(failure) = failure {
        if let Some(object) = status.as_object_mut() {
            object.insert(
                "reason".to_string(),
                serde_json::Value::String(failure.reason.to_string()),
            );
            object.insert(
                "message".to_string(),
                serde_json::Value::String(failure.message.to_string()),
            );
        }
    }
    CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status).unwrap_or_else(|_| b"{}".to_vec()),
        exit_code: Some(if success { 0 } else { 1 }),
        done: true,
    }
}

fn socket_addr(address: &str, port: u16) -> Result<SocketAddr> {
    Ok(SocketAddr::new(ip_addr(address)?, port))
}

fn ip_addr(address: &str) -> Result<IpAddr> {
    address
        .parse()
        .with_context(|| format!("invalid speed-test IP address {address}"))
}

fn server_tunnel_address(plan: &TunnelPlan, server_side: TunnelEndpointSide) -> &str {
    match server_side {
        TunnelEndpointSide::Left => &plan.left_tunnel_address,
        TunnelEndpointSide::Right => &plan.right_tunnel_address,
    }
}

fn side_label(side: TunnelEndpointSide) -> &'static str {
    match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpsman_common::{
        plan_tunnel, BandwidthTier, JobCommand, OspfCostPolicy, TunnelKind, TunnelPlanInput,
    };

    #[tokio::test]
    async fn loopback_speed_test_reports_client_and_server_metrics() {
        let plan = test_loopback_plan();
        let port = unused_loopback_port();
        let left_config = AgentConfig {
            client_id: "left-a".to_string(),
            display_name: "left-a".to_string(),
            ..AgentConfig::default()
        };
        let right_config = AgentConfig {
            client_id: "right-b".to_string(),
            display_name: "right-b".to_string(),
            ..AgentConfig::default()
        };
        let job_id = uuid::Uuid::new_v4();
        let command_payload_hash = test_speed_test_command_hash(&plan, port);

        let server = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id,
            command_payload_hash: &command_payload_hash,
            config: &left_config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 64 * 1024,
            rate_limit_kbps: 1024,
            port,
            connect_timeout_ms: 3000,
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        });
        time::sleep(Duration::from_millis(50)).await;
        let client = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id,
            command_payload_hash: &command_payload_hash,
            config: &right_config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 64 * 1024,
            rate_limit_kbps: 1024,
            port,
            connect_timeout_ms: 3000,
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        });
        let (server_outputs, client_outputs) = tokio::join!(server, client);
        let server_status = status_json(server_outputs.unwrap());
        let client_status = status_json(client_outputs.unwrap());

        assert_eq!(server_status["type"], "network_speed_test");
        assert_eq!(server_status["role"], "server");
        assert_eq!(client_status["role"], "client");
        assert_eq!(server_status["success"], true);
        assert_eq!(client_status["success"], true);
        assert!(server_status["bytes"].as_u64().unwrap() > 0);
        assert!(client_status["throughput_mbps"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn rejects_speed_test_for_non_endpoint_agent() {
        let plan = test_loopback_plan();
        let port = unused_loopback_port();
        let command_payload_hash = test_speed_test_command_hash(&plan, port);
        let config = AgentConfig {
            client_id: "outside".to_string(),
            display_name: "outside".to_string(),
            ..AgentConfig::default()
        };

        let error = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id: uuid::Uuid::new_v4(),
            command_payload_hash: &command_payload_hash,
            config: &config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 16 * 1024,
            rate_limit_kbps: 512,
            port,
            connect_timeout_ms: 200,
            timeout_secs: 1,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("but this agent is outside"));
    }

    #[tokio::test]
    async fn rejects_unbounded_speed_test_budget_on_agent() {
        let plan = test_loopback_plan();
        let port = unused_loopback_port();
        let command_payload_hash = test_speed_test_command_hash(&plan, port);
        let config = AgentConfig {
            client_id: "left-a".to_string(),
            display_name: "left-a".to_string(),
            ..AgentConfig::default()
        };

        let error = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id: uuid::Uuid::new_v4(),
            command_payload_hash: &command_payload_hash,
            config: &config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 31,
            max_bytes: 16 * 1024,
            rate_limit_kbps: 512,
            port,
            connect_timeout_ms: 200,
            timeout_secs: 1,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("duration is out of range"));
    }

    #[tokio::test]
    async fn rejects_speed_test_wrong_nonce() {
        let plan = test_loopback_plan();
        let port = unused_loopback_port();
        let left_config = AgentConfig {
            client_id: "left-a".to_string(),
            display_name: "left-a".to_string(),
            ..AgentConfig::default()
        };
        let right_config = AgentConfig {
            client_id: "right-b".to_string(),
            display_name: "right-b".to_string(),
            ..AgentConfig::default()
        };
        let job_id = uuid::Uuid::new_v4();
        let command_payload_hash = test_speed_test_command_hash(&plan, port);
        let wrong_payload_hash = "0".repeat(64);

        let server = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id,
            command_payload_hash: &command_payload_hash,
            config: &left_config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 16 * 1024,
            rate_limit_kbps: 1024,
            port,
            connect_timeout_ms: 300,
            timeout_secs: 2,
            cancel_token: CommandCancelToken::default(),
        });
        time::sleep(Duration::from_millis(50)).await;
        let client = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id,
            command_payload_hash: &wrong_payload_hash,
            config: &right_config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 16 * 1024,
            rate_limit_kbps: 1024,
            port,
            connect_timeout_ms: 300,
            timeout_secs: 2,
            cancel_token: CommandCancelToken::default(),
        });
        let (server_outputs, client_outputs) = tokio::join!(server, client);
        let server_status = status_json(server_outputs.unwrap());
        let client_status = status_json(client_outputs.unwrap());

        assert_eq!(server_status["success"], false);
        assert_eq!(server_status["reason"], "peer_nonce_mismatch");
        assert_eq!(client_status["success"], false);
        assert_eq!(client_status["reason"], "peer_verification_rejected");
    }

    #[tokio::test]
    async fn rejects_speed_test_wrong_peer_address() {
        let plan = test_loopback_plan();
        let port = unused_loopback_port();
        let left_config = AgentConfig {
            client_id: "left-a".to_string(),
            display_name: "left-a".to_string(),
            ..AgentConfig::default()
        };
        let command_payload_hash = test_speed_test_command_hash(&plan, port);

        let server = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id: uuid::Uuid::new_v4(),
            command_payload_hash: &command_payload_hash,
            config: &left_config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 16 * 1024,
            rate_limit_kbps: 1024,
            port,
            connect_timeout_ms: 300,
            timeout_secs: 2,
            cancel_token: CommandCancelToken::default(),
        });
        let wrong_peer = async {
            time::sleep(Duration::from_millis(50)).await;
            connect_without_source_bind(socket_addr(&plan.left_tunnel_address, port).unwrap()).await
        };
        let (server_outputs, wrong_peer_result) = tokio::join!(server, wrong_peer);
        let _wrong_peer = wrong_peer_result.unwrap();
        let server_status = status_json(server_outputs.unwrap());

        assert_eq!(server_status["success"], false);
        assert_eq!(server_status["reason"], "peer_address_mismatch");
        assert!(server_status["peer_socket"]
            .as_str()
            .unwrap()
            .starts_with("127.0.0.1:"));
    }

    fn status_json(outputs: Vec<CommandOutput>) -> serde_json::Value {
        serde_json::from_slice(&outputs[0].data).unwrap()
    }

    async fn connect_without_source_bind(target_addr: SocketAddr) -> std::io::Result<TcpStream> {
        let started = Instant::now();
        loop {
            match TcpStream::connect(target_addr).await {
                Ok(stream) => return Ok(stream),
                Err(error) if started.elapsed() < Duration::from_secs(1) => {
                    time::sleep(Duration::from_millis(25)).await;
                    if started.elapsed() >= Duration::from_secs(1) {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn unused_loopback_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn test_speed_test_command_hash(plan: &TunnelPlan, port: u16) -> String {
        let command = JobCommand::NetworkSpeedTest {
            plan: Box::new(plan.clone()),
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 64 * 1024,
            rate_limit_kbps: 1024,
            port,
            connect_timeout_ms: 3000,
        };
        payload_hash(&serde_json::to_vec(&command).unwrap())
    }

    fn test_loopback_plan() -> TunnelPlan {
        plan_tunnel(&TunnelPlanInput {
            name: "left-right".to_string(),
            interface_name: "tunlr".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "left-a".to_string(),
            right_client_id: "right-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "127.0.0.0/29".to_string(),
            reserved_addresses: vec!["127.0.0.0".to_string(), "127.0.0.1".to_string()],
            ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "127.0.0.2".to_string(),
                right: "127.0.0.3".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 15.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap()
    }
}
