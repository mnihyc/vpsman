mod api_client;
mod build_info;
mod control;
mod state;

use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tracing::{debug, info, warn};
use vpsman_common::{
    decode_json, decode_noise_key_hex, encode_json, AgentHello, Frame, GatewayAgentHelloIngest,
    GatewayCommandOutputIngest, GatewaySessionLifecycleIngest, GatewayTelemetryIngest,
    GatewayTerminalOutputIngest, JobAck, MessageKind, NoiseFrameStream, ServerHello,
    TelemetryEnvelope,
};

use crate::{
    api_client::GatewayControlClient,
    control::run_control_listener,
    state::{finish_pending_command, GatewayCommand, GatewaySession, GatewayState, PendingCommand},
};

#[derive(Clone, Debug, Parser)]
#[command(name = "vpsman-gateway", about = "TCP gateway for VPS agents")]
pub(crate) struct Args {
    #[arg(long, env = "VPSMAN_GATEWAY_BIND", default_value = "0.0.0.0:9443")]
    bind: String,
    #[arg(
        long,
        env = "VPSMAN_GATEWAY_CONTROL_BIND",
        default_value = "127.0.0.1:9444"
    )]
    control_bind: String,
    #[arg(long, env = "VPSMAN_GATEWAY_PRIVATE_KEY_HEX")]
    private_key_hex: Option<String>,
    #[arg(long, env = "VPSMAN_GATEWAY_EXPECT_CLIENT_PUBLIC_KEY_HEX")]
    expect_client_public_key_hex: Option<String>,
    #[arg(long, env = "VPSMAN_API_URL")]
    api_url: Option<String>,
    #[arg(long, env = "VPSMAN_INTERNAL_TOKEN")]
    internal_token: Option<String>,
    #[arg(long, env = "VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX")]
    privilege_verifier_key_hex: Option<String>,
    #[arg(long, env = "VPSMAN_GATEWAY_ID", default_value = "local-dev-gateway")]
    gateway_id: String,
    #[arg(
        long,
        env = "VPSMAN_GATEWAY_RECONNECT_GRACE_SECS",
        default_value_t = 60
    )]
    reconnect_grace_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vpsman_gateway=info".into()),
        )
        .init();

    let mut args = Args::parse();
    info!(
        version = env!("CARGO_PKG_VERSION"),
        server_build_number = build_info::server_build_number(),
        "gateway build metadata"
    );
    args.internal_token = Some(required_internal_token(args.internal_token.as_deref())?);
    validate_gateway_runtime_mode(&args)?;
    let state = GatewayState {
        reconnect_grace_secs: args.reconnect_grace_secs,
        ..GatewayState::default()
    };
    let agent_args = args.clone();
    let agent_state = state.clone();
    let control_args = args.clone();
    let control_state = state.clone();

    tokio::try_join!(
        run_agent_listener(agent_args, agent_state),
        run_control_listener(control_args, control_state),
    )?;
    Ok(())
}

fn required_internal_token(value: Option<&str>) -> Result<String> {
    let token = value
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .context("VPSMAN_INTERNAL_TOKEN is required")?;
    anyhow::ensure!(
        token.len() >= 32,
        "VPSMAN_INTERNAL_TOKEN must be at least 32 characters"
    );
    anyhow::ensure!(
        !matches!(
            token,
            "change-me"
                | "change-me-internal-token"
                | "replace-with-random-token-at-least-32-chars"
        ),
        "VPSMAN_INTERNAL_TOKEN must be changed from the deployment template placeholder"
    );
    Ok(token.to_string())
}

fn validate_gateway_runtime_mode(args: &Args) -> Result<()> {
    let private_key = args
        .private_key_hex
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("VPSMAN_GATEWAY_PRIVATE_KEY_HEX is required")?;
    let private_key_bytes =
        hex::decode(private_key).context("VPSMAN_GATEWAY_PRIVATE_KEY_HEX must be hex")?;
    anyhow::ensure!(
        private_key_bytes.len() == 32,
        "VPSMAN_GATEWAY_PRIVATE_KEY_HEX must be a 32-byte hex key"
    );

    let verifier_key = args
        .privilege_verifier_key_hex
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if verifier_key.is_none() {
        anyhow::bail!("VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX is required");
    }
    if let Some(verifier_key) = verifier_key {
        let bytes =
            hex::decode(verifier_key).context("VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX must be hex")?;
        anyhow::ensure!(
            bytes.len() == 32,
            "VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX must be a 32-byte hex key"
        );
    }
    Ok(())
}

async fn run_agent_listener(args: Args, state: GatewayState) -> Result<()> {
    let listener = TcpListener::bind(&args.bind)
        .await
        .with_context(|| format!("failed to bind gateway on {}", args.bind))?;
    info!(bind = %args.bind, "gateway listening");

    loop {
        let (stream, peer) = listener.accept().await?;
        info!(%peer, "accepted agent connection");
        let args = args.clone();
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_agent(stream, peer, args, state).await {
                warn!(%peer, %error, "agent session ended with error");
            }
        });
    }
}

async fn handle_agent(
    stream: TcpStream,
    peer: SocketAddr,
    args: Args,
    state: GatewayState,
) -> Result<()> {
    let mut stream = accept_noise_stream(stream, &args).await?;
    let control = GatewayControlClient::new(args.api_url.clone(), args.internal_token.clone());
    let noise_public_key_hex = stream.remote_static().map(hex::encode);
    let remote_ip = peer.ip().to_string();
    let session_id = uuid::Uuid::new_v4();
    let mut client_id = None::<String>;
    let (command_tx, mut command_rx) = mpsc::channel::<GatewayCommand>(8);
    let mut outbound_seq = 2_u64;
    let mut pending_command = None::<PendingCommand>;

    let result: Result<()> = loop {
        tokio::select! {
            frame = stream.read_frame() => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(error) => break Err(error.into()),
                };
                let context = AgentFrameContext {
                    args: &args,
                    state: &state,
                    control: &control,
                    noise_public_key_hex: noise_public_key_hex.clone(),
                    remote_ip: &remote_ip,
                    session_id,
                    command_tx: &command_tx,
                };
                if let Err(error) =
                    handle_agent_frame(&mut stream, context, &mut client_id, &mut pending_command, frame).await
                {
                    break Err(error);
                }
            }
            command = command_rx.recv(), if pending_command.is_none() && client_id.is_some() => {
                let Some(command) = command else {
                    continue;
                };
                let client_id = client_id.clone().unwrap_or_default();
                let job_id = command.request.job_id;
                let command_version = command.request.command_version;
                if let Err(error) = write_json_frame(
                    &mut stream,
                    MessageKind::Command,
                    1,
                    outbound_seq,
                    &command.request,
                )
                .await
                {
                    break Err(error);
                }
                outbound_seq += 1;
                pending_command = Some(PendingCommand {
                    client_id,
                    job_id,
                    command_version,
                    ack: None,
                    outputs: Vec::new(),
                    next_output_seq: 0,
                    response: command.response,
                });
            }
        }
    };

    if let Some(client_id) = client_id {
        unregister_session_if_current(&state, &client_id, session_id).await;
        state
            .disconnected_at
            .write()
            .await
            .insert(client_id.clone(), std::time::Instant::now());
        let end_event = GatewaySessionLifecycleIngest {
            gateway_id: args.gateway_id.clone(),
            client_id,
            session_id,
            noise_public_key_hex,
            remote_ip: Some(remote_ip),
            reason: result.as_ref().err().map(session_end_reason),
        };
        control
            .post("/internal/v1/gateway/session-ended", &end_event)
            .await;
    }
    result
}

async fn unregister_session_if_current(
    state: &GatewayState,
    client_id: &str,
    session_id: uuid::Uuid,
) {
    let mut sessions = state.sessions.write().await;
    if sessions
        .get(client_id)
        .is_some_and(|session| session.session_id == session_id)
    {
        sessions.remove(client_id);
    }
}

struct AgentFrameContext<'a> {
    args: &'a Args,
    state: &'a GatewayState,
    control: &'a GatewayControlClient,
    noise_public_key_hex: Option<String>,
    remote_ip: &'a str,
    session_id: uuid::Uuid,
    command_tx: &'a mpsc::Sender<GatewayCommand>,
}

async fn handle_agent_frame(
    stream: &mut NoiseFrameStream<TcpStream>,
    context: AgentFrameContext<'_>,
    client_id: &mut Option<String>,
    pending_command: &mut Option<PendingCommand>,
    frame: Frame,
) -> Result<()> {
    match frame.kind {
        MessageKind::ClientHello => {
            let hello: AgentHello = decode_json(&frame.decoded_payload()?)?;
            validate_runtime_identity(&hello, &context).await?;
            info!(
                client_id = %hello.client_id,
                arch = %hello.arch,
                "agent hello"
            );
            let ingest = GatewayAgentHelloIngest {
                gateway_id: context.args.gateway_id.clone(),
                noise_public_key_hex: context.noise_public_key_hex.clone(),
                remote_ip: Some(context.remote_ip.to_string()),
                hello: hello.clone(),
            };
            context
                .control
                .post("/internal/v1/gateway/agent-hello", &ingest)
                .await;
            *client_id = Some(hello.client_id.clone());
            context.state.sessions.write().await.insert(
                hello.client_id.clone(),
                GatewaySession {
                    session_id: context.session_id,
                    sender: context.command_tx.clone(),
                },
            );
            context
                .state
                .disconnected_at
                .write()
                .await
                .remove(&hello.client_id);
            let session_event = GatewaySessionLifecycleIngest {
                gateway_id: context.args.gateway_id.clone(),
                client_id: hello.client_id.clone(),
                session_id: context.session_id,
                noise_public_key_hex: context.noise_public_key_hex.clone(),
                remote_ip: Some(context.remote_ip.to_string()),
                reason: None,
            };
            context
                .control
                .post("/internal/v1/gateway/session-started", &session_event)
                .await;
            let reply = ServerHello {
                server_id: context.args.gateway_id.clone(),
                server_version: env!("CARGO_PKG_VERSION").to_string(),
                server_build_number: crate::build_info::server_build_number(),
                accepted: true,
                message: "accepted".to_string(),
                telemetry_light_secs: 15,
                telemetry_full_secs: 60,
            };
            write_json_frame(stream, MessageKind::ServerHello, 0, frame.seq, &reply).await?;
        }
        MessageKind::Telemetry => {
            let telemetry: TelemetryEnvelope = decode_json(&frame.decoded_payload()?)?;
            info!(
                client_id = %telemetry.client_id,
                hostname = %telemetry.metrics.hostname,
                uptime = telemetry.metrics.uptime_secs,
                "telemetry received"
            );
            let ingest = GatewayTelemetryIngest {
                gateway_id: context.args.gateway_id.clone(),
                remote_ip: Some(context.remote_ip.to_string()),
                telemetry,
            };
            context
                .control
                .post("/internal/v1/gateway/telemetry", &ingest)
                .await;
        }
        MessageKind::CommandAck => {
            let ack: JobAck = decode_json(&frame.decoded_payload()?)?;
            if let Some(pending) = pending_command.as_mut() {
                if pending.job_id == ack.job_id {
                    if !ack.accepted {
                        finish_pending_command(pending_command, Some(ack), Vec::new());
                    } else {
                        pending.ack = Some(ack);
                    }
                }
            }
        }
        MessageKind::CommandOutput => {
            let output: vpsman_common::CommandOutput = decode_json(&frame.decoded_payload()?)?;
            if let Some(pending) = pending_command.as_mut() {
                if pending.job_id == output.job_id {
                    let done = output.done;
                    let seq = pending.next_output_seq;
                    pending.next_output_seq = pending.next_output_seq.saturating_add(1);
                    let ingest = GatewayCommandOutputIngest {
                        gateway_id: context.args.gateway_id.clone(),
                        client_id: pending.client_id.clone(),
                        job_id: output.job_id,
                        seq,
                        output: output.clone(),
                    };
                    context
                        .control
                        .post("/internal/v1/gateway/command-output", &ingest)
                        .await;
                    pending.outputs.push(output);
                    if done {
                        finish_pending_command(pending_command, None, Vec::new());
                    }
                }
            }
        }
        MessageKind::TerminalStreamOutput => {
            let output: vpsman_common::TerminalStreamOutput =
                decode_json(&frame.decoded_payload()?)?;
            let Some(client_id) = client_id.clone() else {
                return Ok(());
            };
            let ingest = GatewayTerminalOutputIngest {
                gateway_id: context.args.gateway_id.clone(),
                client_id,
                output,
            };
            context
                .control
                .post("/internal/v1/gateway/terminal-output", &ingest)
                .await;
        }
        MessageKind::Keepalive => {
            debug!(?client_id, "keepalive");
        }
        other => {
            debug!(?other, "unhandled gateway frame");
        }
    }
    Ok(())
}

fn session_end_reason(error: &anyhow::Error) -> String {
    let mut reason = error.to_string();
    reason.truncate(512);
    reason
}

async fn accept_noise_stream(
    stream: TcpStream,
    args: &Args,
) -> Result<NoiseFrameStream<TcpStream>> {
    let private_key = args
        .private_key_hex
        .as_deref()
        .context("gateway requires --private-key-hex")?;
    let private_key = decode_noise_key_hex(private_key)?;
    let expected_client_public_key = args
        .expect_client_public_key_hex
        .as_deref()
        .map(decode_noise_key_hex)
        .transpose()?;
    NoiseFrameStream::server_enrolled(stream, &private_key, expected_client_public_key.as_deref())
        .await
        .map_err(Into::into)
}

async fn validate_runtime_identity(
    hello: &AgentHello,
    context: &AgentFrameContext<'_>,
) -> Result<()> {
    let public_key_hex = context
        .noise_public_key_hex
        .as_deref()
        .context("enrolled IK session did not expose client static key")?;
    if context.args.expect_client_public_key_hex.is_some() && context.args.api_url.is_none() {
        return Ok(());
    }
    let validation = context
        .control
        .validate_agent_identity(&hello.client_id, public_key_hex)
        .await?;
    if validation.accepted {
        Ok(())
    } else {
        anyhow::bail!(
            "agent identity rejected for {}: {}",
            hello.client_id,
            validation.message
        )
    }
}

async fn write_json_frame<T: serde::Serialize>(
    stream: &mut NoiseFrameStream<TcpStream>,
    kind: MessageKind,
    stream_id: u32,
    seq: u64,
    value: &T,
) -> Result<()> {
    let frame = Frame::new(kind, stream_id, seq, encode_json(value)?);
    stream.write_frame(&frame).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stale_reconnect_cleanup_does_not_remove_newer_session() {
        let state = GatewayState::default();
        let older_session_id = uuid::Uuid::new_v4();
        let newer_session_id = uuid::Uuid::new_v4();
        let (older_tx, _older_rx) = mpsc::channel(1);
        let (newer_tx, _newer_rx) = mpsc::channel(1);
        state.sessions.write().await.insert(
            "client-a".to_string(),
            GatewaySession {
                session_id: older_session_id,
                sender: older_tx,
            },
        );
        state.sessions.write().await.insert(
            "client-a".to_string(),
            GatewaySession {
                session_id: newer_session_id,
                sender: newer_tx,
            },
        );

        unregister_session_if_current(&state, "client-a", older_session_id).await;
        assert_eq!(
            state
                .sessions
                .read()
                .await
                .get("client-a")
                .map(|session| session.session_id),
            Some(newer_session_id)
        );

        unregister_session_if_current(&state, "client-a", newer_session_id).await;
        assert!(!state.sessions.read().await.contains_key("client-a"));
    }

    #[test]
    fn internal_token_startup_validation_rejects_missing_short_or_placeholder() {
        assert!(required_internal_token(None).is_err());
        assert!(required_internal_token(Some("short")).is_err());
        assert!(required_internal_token(Some("change-me-internal-token")).is_err());
        assert!(
            required_internal_token(Some("replace-with-random-token-at-least-32-chars")).is_err()
        );
        assert!(required_internal_token(Some("real-internal-token-value-32-plus-chars")).is_ok());
    }

    #[test]
    fn runtime_mode_requires_identity_key_and_privilege_verifier() {
        let mut args = test_args();

        args.private_key_hex = None;
        assert!(validate_gateway_runtime_mode(&args)
            .unwrap_err()
            .to_string()
            .contains("VPSMAN_GATEWAY_PRIVATE_KEY_HEX is required"));

        args.private_key_hex = Some("11".repeat(32));
        args.privilege_verifier_key_hex = None;
        assert!(validate_gateway_runtime_mode(&args)
            .unwrap_err()
            .to_string()
            .contains("VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX is required"));

        args.privilege_verifier_key_hex = Some("11".repeat(32));
        validate_gateway_runtime_mode(&args).unwrap();
    }

    fn test_args() -> Args {
        Args {
            bind: "127.0.0.1:0".to_string(),
            control_bind: "127.0.0.1:0".to_string(),
            private_key_hex: Some("11".repeat(32)),
            expect_client_public_key_hex: None,
            api_url: None,
            internal_token: Some("real-internal-token-value-32-plus-chars".to_string()),
            privilege_verifier_key_hex: Some("11".repeat(32)),
            gateway_id: "test-gateway".to_string(),
            reconnect_grace_secs: 60,
        }
    }
}
