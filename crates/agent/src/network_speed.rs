use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time,
};
use vpsman_common::{
    render_tunnel_endpoint_config, AgentConfig, CommandOutput, OutputStream, TunnelEndpointSide,
    TunnelPlan, NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS, NETWORK_SPEED_TEST_MAX_DURATION_SECS,
    NETWORK_SPEED_TEST_MAX_MAX_BYTES, NETWORK_SPEED_TEST_MAX_PORT,
    NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS, NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS,
    NETWORK_SPEED_TEST_MIN_DURATION_SECS, NETWORK_SPEED_TEST_MIN_MAX_BYTES,
    NETWORK_SPEED_TEST_MIN_PORT, NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS,
};

const SPEED_CHUNK_BYTES: usize = 16 * 1024;
const CONNECT_RETRY_MS: u64 = 100;

pub(crate) struct NetworkSpeedTestInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) server_side: TunnelEndpointSide,
    pub(crate) duration_secs: u8,
    pub(crate) max_bytes: u64,
    pub(crate) rate_limit_kbps: u32,
    pub(crate) port: u16,
    pub(crate) connect_timeout_ms: u16,
    pub(crate) timeout_secs: u64,
}

pub(crate) async fn execute_network_speed_test_command(
    input: NetworkSpeedTestInput<'_>,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        run_network_speed_test(input),
    )
    .await
    .context("network speed test timed out")?
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
            plan: input.plan,
            client_id: &input.config.client_id,
            peer_client_id: &server_endpoint.peer_client_id,
            role: "server",
            server_side: input.server_side,
            server_address,
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
            plan: input.plan,
            client_id: &input.config.client_id,
            peer_client_id: &server_endpoint.local_client_id,
            role: "client",
            server_side: input.server_side,
            server_address,
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
    plan: &'a TunnelPlan,
    client_id: &'a str,
    peer_client_id: &'a str,
    role: &'static str,
    server_side: TunnelEndpointSide,
    server_address: &'a str,
    port: u16,
    duration: Duration,
    max_bytes: u64,
    rate_limit_kbps: u32,
    connect_timeout: Duration,
}

async fn receive_speed_test(input: NetworkSpeedRoleInput<'_>) -> Result<CommandOutput> {
    let bind_addr = socket_addr(input.server_address, input.port)?;
    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("failed to bind speed-test listener at {bind_addr}"))?;
    let started = Instant::now();
    let (mut stream, peer_addr) = time::timeout(input.connect_timeout, listener.accept())
        .await
        .context("network speed test listener timed out waiting for peer")?
        .context("failed to accept speed-test peer")?;
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
    ))
}

async fn send_speed_test(input: NetworkSpeedRoleInput<'_>) -> Result<CommandOutput> {
    let target_addr = socket_addr(input.server_address, input.port)?;
    let mut stream = connect_with_retry(target_addr, input.connect_timeout).await?;
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
    ))
}

async fn connect_with_retry(target_addr: SocketAddr, timeout: Duration) -> Result<TcpStream> {
    let started = Instant::now();
    loop {
        match TcpStream::connect(target_addr).await {
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

fn status_output(
    input: NetworkSpeedRoleInput<'_>,
    peer_addr: Option<SocketAddr>,
    bytes: u64,
    elapsed: Duration,
    success: bool,
) -> CommandOutput {
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    let mbps = if elapsed.as_secs_f64() > 0.0 {
        (bytes as f64 * 8.0) / elapsed.as_secs_f64() / 1_000_000.0
    } else {
        0.0
    };
    let status = serde_json::json!({
        "type": "network_speed_test",
        "probe": "tcp_throughput",
        "role": input.role,
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "server_side": side_label(input.server_side),
        "client_id": input.client_id,
        "peer_client_id": input.peer_client_id,
        "server_address": input.server_address,
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
    CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status).unwrap_or_else(|_| b"{}".to_vec()),
        exit_code: Some(if success { 0 } else { 1 }),
        done: true,
    }
}

fn socket_addr(address: &str, port: u16) -> Result<SocketAddr> {
    format!("{address}:{port}")
        .parse()
        .with_context(|| format!("invalid speed-test socket address {address}:{port}"))
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
    use vpsman_common::{plan_tunnel, BandwidthTier, OspfCostPolicy, TunnelKind, TunnelPlanInput};

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

        let server = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id,
            config: &left_config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 64 * 1024,
            rate_limit_kbps: 1024,
            port,
            connect_timeout_ms: 3000,
            timeout_secs: 5,
        });
        time::sleep(Duration::from_millis(50)).await;
        let client = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id,
            config: &right_config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 64 * 1024,
            rate_limit_kbps: 1024,
            port,
            connect_timeout_ms: 3000,
            timeout_secs: 5,
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
        let config = AgentConfig {
            client_id: "outside".to_string(),
            display_name: "outside".to_string(),
            ..AgentConfig::default()
        };

        let error = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id: uuid::Uuid::new_v4(),
            config: &config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 1,
            max_bytes: 16 * 1024,
            rate_limit_kbps: 512,
            port: unused_loopback_port(),
            connect_timeout_ms: 200,
            timeout_secs: 1,
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("but this agent is outside"));
    }

    #[tokio::test]
    async fn rejects_unbounded_speed_test_budget_on_agent() {
        let plan = test_loopback_plan();
        let config = AgentConfig {
            client_id: "left-a".to_string(),
            display_name: "left-a".to_string(),
            ..AgentConfig::default()
        };

        let error = execute_network_speed_test_command(NetworkSpeedTestInput {
            job_id: uuid::Uuid::new_v4(),
            config: &config,
            plan: &plan,
            server_side: TunnelEndpointSide::Left,
            duration_secs: 31,
            max_bytes: 16 * 1024,
            rate_limit_kbps: 512,
            port: unused_loopback_port(),
            connect_timeout_ms: 200,
            timeout_secs: 1,
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("duration is out of range"));
    }

    fn status_json(outputs: Vec<CommandOutput>) -> serde_json::Value {
        serde_json::from_slice(&outputs[0].data).unwrap()
    }

    fn unused_loopback_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
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
            bandwidth: BandwidthTier::M100,
            latency_ms: 15.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap()
    }
}
