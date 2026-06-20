mod api_client;
mod build_info;
mod control;
mod state;

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore},
    time,
};
use tracing::{debug, info, warn};
use vpsman_common::{
    decode_json, decode_noise_key_hex, encode_json, read_secret_file_ref, AgentHello,
    CommandResume, Frame, GatewayAgentHelloIngest, GatewayCommandOutputIngest,
    GatewaySessionLifecycleIngest, GatewayTelemetryIngest, GatewayTerminalOutputIngest, JobAck,
    JobCancelAck, MessageKind, NoiseFrameStream, SequencedCommandOutput, ServerHello, SuiteConfig,
    TelemetryEnvelope,
};

use crate::{
    api_client::{
        GatewayControlClient, GatewayForwardConfig, GatewayHttpTimeouts, GatewaySpoolConfig,
        DEFAULT_COMMAND_OUTPUT_EVENT_TTL_SECS,
    },
    control::run_control_listener,
    state::{
        cancel_ack_result, finish_pending_command_response, GatewaySession, GatewaySessionMessage,
        GatewayState, PendingCommand, SESSION_COMMAND_QUEUE_CAPACITY,
    },
};

const MAX_AGENT_CONNECTIONS: usize = 4096;

#[derive(Clone, Debug, Parser)]
#[command(name = "vpsman-gateway", about = "TCP gateway for VPS agents")]
pub(crate) struct Args {
    #[arg(
        long,
        env = "VPSMAN_SUITE_CONFIG",
        default_value = "config/vpsman.toml"
    )]
    suite_config: PathBuf,
    #[arg(long, env = "VPSMAN_GATEWAY_BIND", default_value = "127.0.0.1:9443")]
    bind: String,
    #[arg(
        long,
        env = "VPSMAN_GATEWAY_CONTROL_BIND",
        default_value = "unix:./runtime/gateway-control.sock"
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
    #[arg(long, env = "VPSMAN_INTERNAL_HTTP_CONNECT_SECS", default_value_t = 10)]
    internal_http_connect_secs: u64,
    #[arg(long, env = "VPSMAN_INTERNAL_HTTP_WRITE_SECS", default_value_t = 10)]
    internal_http_write_secs: u64,
    #[arg(long, env = "VPSMAN_INTERNAL_HTTP_READ_SECS", default_value_t = 15)]
    internal_http_read_secs: u64,
    #[arg(long, env = "VPSMAN_EVENT_POST_SECS", default_value_t = 15)]
    event_post_secs: u64,
    #[arg(long, env = "VPSMAN_DISPATCH_ACK_SECS", default_value_t = 30)]
    dispatch_ack_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_GATEWAY_SPOOL_DIR",
        default_value = "./runtime/gateway-spool"
    )]
    spool_dir: PathBuf,
    #[arg(
        long,
        env = "VPSMAN_GATEWAY_SPOOL_RAM_MAX_BYTES",
        default_value_t = 1024 * 1024 * 1024
    )]
    spool_ram_max_bytes: u64,
    #[arg(
        long,
        env = "VPSMAN_GATEWAY_SPOOL_DISK_MAX_BYTES",
        default_value_t = 4 * 1024 * 1024 * 1024
    )]
    spool_disk_max_bytes: u64,
    #[arg(
        long,
        env = "VPSMAN_GATEWAY_SPOOL_SHUTDOWN_FLUSH_SECS",
        default_value_t = 30
    )]
    spool_shutdown_flush_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS",
        default_value_t = DEFAULT_COMMAND_OUTPUT_EVENT_TTL_SECS
    )]
    command_output_event_ttl_secs: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GatewayRuntimeConfig {
    reconnect_grace_secs: u64,
    dispatch_ack_secs: u64,
    http_timeouts: GatewayHttpTimeouts,
    forward_config: GatewayForwardConfig,
}

impl GatewayRuntimeConfig {
    fn from_args(args: &Args) -> Self {
        Self {
            reconnect_grace_secs: args.reconnect_grace_secs.clamp(1, 3600),
            dispatch_ack_secs: args.dispatch_ack_secs.clamp(1, 3600),
            http_timeouts: args.gateway_http_timeouts(),
            forward_config: args.gateway_forward_config(),
        }
    }
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
    let base_args = args.clone();
    let suite_config =
        SuiteConfig::load_optional(&args.suite_config).map_err(anyhow::Error::msg)?;
    args.apply_suite_config(&suite_config)
        .map_err(anyhow::Error::msg)?;
    info!(
        version = build_info::release_version(),
        server_build_number = build_info::server_build_number(),
        "gateway build metadata"
    );
    args.internal_token = Some(required_internal_token(args.internal_token.as_deref())?);
    validate_gateway_runtime_mode(&args)?;
    let runtime_config = GatewayRuntimeConfig::from_args(&args);
    let api_client = GatewayControlClient::new_with_spool(
        args.api_url.clone(),
        args.internal_token.clone(),
        runtime_config.http_timeouts,
        args.gateway_spool_config(),
        runtime_config.forward_config,
    );
    let state = GatewayState {
        forward_metrics: api_client.forward_metrics(),
        ..GatewayState::default()
    };
    state.set_runtime_timing(
        runtime_config.reconnect_grace_secs,
        runtime_config.dispatch_ack_secs,
    );
    spawn_gateway_runtime_config_reloader(
        base_args,
        runtime_config,
        state.clone(),
        api_client.clone(),
    );
    let critical_failure_state = state.clone();
    api_client.set_critical_failure_handler(move |client_id, reason| {
        let state = critical_failure_state.clone();
        tokio::spawn(async move {
            request_agent_disconnect(&state, &client_id, reason).await;
        });
    });
    let agent_args = args.clone();
    let agent_state = state.clone();
    let agent_api_client = api_client.clone();
    let control_args = args.clone();
    let control_state = state.clone();
    let shutdown_state = state.clone();
    let shutdown_api_client = api_client.clone();
    let shutdown_flush = args.gateway_spool_config().shutdown_flush;

    tokio::select! {
        result = run_agent_listener(agent_args, agent_state, agent_api_client) => result?,
        result = run_control_listener(control_args, control_state) => result?,
        _ = shutdown_signal() => {
            request_all_agent_disconnects(&shutdown_state, "gateway_shutdown").await;
            shutdown_api_client.shutdown_flush(shutdown_flush).await;
        }
    }
    Ok(())
}

impl Args {
    fn apply_suite_config(&mut self, config: &SuiteConfig) -> std::result::Result<(), String> {
        apply_string_default(
            &mut self.bind,
            "VPSMAN_GATEWAY_BIND",
            config.gateway.bind.as_deref(),
        );
        apply_string_default(
            &mut self.control_bind,
            "VPSMAN_GATEWAY_CONTROL_BIND",
            config.gateway.control_bind.as_deref(),
        );
        apply_opt_string(
            &mut self.expect_client_public_key_hex,
            "VPSMAN_GATEWAY_EXPECT_CLIENT_PUBLIC_KEY_HEX",
            config.gateway.expect_client_public_key_hex.as_deref(),
        );
        apply_opt_string(
            &mut self.api_url,
            "VPSMAN_API_URL",
            config.gateway.api_url.as_deref(),
        );
        apply_string_default(
            &mut self.gateway_id,
            "VPSMAN_GATEWAY_ID",
            config.gateway.gateway_id.as_deref(),
        );
        self.apply_runtime_suite_config(config);
        if self.internal_token.is_none() && env_absent("VPSMAN_INTERNAL_TOKEN") {
            self.internal_token =
                read_secret_file_ref(config.secrets.internal_token_file.as_deref())?;
        }
        if self.private_key_hex.is_none() && env_absent("VPSMAN_GATEWAY_PRIVATE_KEY_HEX") {
            self.private_key_hex =
                read_secret_file_ref(config.secrets.gateway_private_key_file.as_deref())?;
        }
        if self.privilege_verifier_key_hex.is_none()
            && env_absent("VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX")
        {
            self.privilege_verifier_key_hex =
                read_secret_file_ref(config.secrets.privilege_verifier_key_file.as_deref())?;
        }
        Ok(())
    }

    fn apply_runtime_suite_config(&mut self, config: &SuiteConfig) {
        if env_absent("VPSMAN_GATEWAY_RECONNECT_GRACE_SECS") {
            if let Some(value) = config
                .gateway
                .reconnect_grace_secs
                .or(config.timeout.gateway_reconnect_grace_secs)
            {
                self.reconnect_grace_secs = value;
            }
        }
        apply_u64_default(
            &mut self.internal_http_connect_secs,
            "VPSMAN_INTERNAL_HTTP_CONNECT_SECS",
            config.timeout.internal_http_connect_secs,
        );
        apply_u64_default(
            &mut self.internal_http_write_secs,
            "VPSMAN_INTERNAL_HTTP_WRITE_SECS",
            config.timeout.internal_http_write_secs,
        );
        apply_u64_default(
            &mut self.internal_http_read_secs,
            "VPSMAN_INTERNAL_HTTP_READ_SECS",
            config.timeout.internal_http_read_secs,
        );
        apply_u64_default(
            &mut self.event_post_secs,
            "VPSMAN_EVENT_POST_SECS",
            config.timeout.event_post_secs,
        );
        apply_u64_default(
            &mut self.dispatch_ack_secs,
            "VPSMAN_DISPATCH_ACK_SECS",
            config.timeout.dispatch_ack_secs,
        );
        apply_path_default(
            &mut self.spool_dir,
            "VPSMAN_GATEWAY_SPOOL_DIR",
            config.gateway.spool_dir.as_deref(),
        );
        apply_u64_default(
            &mut self.spool_ram_max_bytes,
            "VPSMAN_GATEWAY_SPOOL_RAM_MAX_BYTES",
            config.gateway.spool_ram_max_bytes,
        );
        apply_u64_default(
            &mut self.spool_disk_max_bytes,
            "VPSMAN_GATEWAY_SPOOL_DISK_MAX_BYTES",
            config.gateway.spool_disk_max_bytes,
        );
        apply_u64_default(
            &mut self.spool_shutdown_flush_secs,
            "VPSMAN_GATEWAY_SPOOL_SHUTDOWN_FLUSH_SECS",
            config.gateway.spool_shutdown_flush_secs,
        );
        apply_u64_default(
            &mut self.command_output_event_ttl_secs,
            "VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS",
            config.gateway.command_output_event_ttl_secs,
        );
    }

    fn gateway_http_timeouts(&self) -> GatewayHttpTimeouts {
        GatewayHttpTimeouts {
            connect: Duration::from_secs(self.internal_http_connect_secs.clamp(1, 300)),
            write: Duration::from_secs(self.internal_http_write_secs.clamp(1, 300)),
            read: Duration::from_secs(self.internal_http_read_secs.clamp(1, 3600)),
            event_post: Duration::from_secs(self.event_post_secs.clamp(1, 3600)),
        }
    }

    fn gateway_spool_config(&self) -> GatewaySpoolConfig {
        GatewaySpoolConfig::enabled(
            self.spool_dir.clone(),
            self.spool_ram_max_bytes,
            self.spool_disk_max_bytes,
            self.spool_shutdown_flush_secs,
        )
    }

    fn gateway_forward_config(&self) -> GatewayForwardConfig {
        GatewayForwardConfig::new(self.command_output_event_ttl_secs)
    }
}

fn load_gateway_runtime_config(base_args: &Args) -> Result<GatewayRuntimeConfig> {
    let mut args = base_args.clone();
    let suite_config =
        SuiteConfig::load_optional(&args.suite_config).map_err(anyhow::Error::msg)?;
    args.apply_runtime_suite_config(&suite_config);
    Ok(GatewayRuntimeConfig::from_args(&args))
}

fn spawn_gateway_runtime_config_reloader(
    base_args: Args,
    mut current: GatewayRuntimeConfig,
    state: GatewayState,
    api_client: GatewayControlClient,
) {
    tokio::spawn(async move {
        let mut ticker = time::interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            match load_gateway_runtime_config(&base_args) {
                Ok(next) if next != current => {
                    state.set_runtime_timing(next.reconnect_grace_secs, next.dispatch_ack_secs);
                    api_client.set_timeouts(next.http_timeouts);
                    api_client.set_forward_config(next.forward_config);
                    current = next;
                    info!(
                        reconnect_grace_secs = next.reconnect_grace_secs,
                        dispatch_ack_secs = next.dispatch_ack_secs,
                        internal_http_connect_secs = next.http_timeouts.connect.as_secs(),
                        internal_http_write_secs = next.http_timeouts.write.as_secs(),
                        internal_http_read_secs = next.http_timeouts.read.as_secs(),
                        event_post_secs = next.http_timeouts.event_post.as_secs(),
                        command_output_event_ttl_secs =
                            next.forward_config.command_output_event_ttl_secs,
                        "gateway runtime suite config hot-reloaded"
                    );
                }
                Ok(_) => {}
                Err(error) => warn!(
                    %error,
                    "failed to hot-reload gateway runtime suite config; keeping current runtime config"
                ),
            }
        }
    });
}

fn env_absent(name: &str) -> bool {
    std::env::var_os(name).is_none()
}

fn apply_opt_string(target: &mut Option<String>, env_name: &str, value: Option<&str>) {
    if target.is_none() && env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = Some(value.to_string());
        }
    }
}

fn apply_string_default(target: &mut String, env_name: &str, value: Option<&str>) {
    if env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = value.to_string();
        }
    }
}

fn apply_path_default(target: &mut PathBuf, env_name: &str, value: Option<&str>) {
    if env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = PathBuf::from(value);
        }
    }
}

fn apply_u64_default(target: &mut u64, env_name: &str, value: Option<u64>) {
    if env_absent(env_name) {
        if let Some(value) = value {
            *target = value;
        }
    }
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
                | "dev-internal-token-change-me-32chars"
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
    args.api_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("VPSMAN_API_URL is required")?;
    Ok(())
}

async fn run_agent_listener(
    args: Args,
    state: GatewayState,
    api_client: GatewayControlClient,
) -> Result<()> {
    let listener = TcpListener::bind(&args.bind)
        .await
        .with_context(|| format!("failed to bind gateway on {}", args.bind))?;
    info!(bind = %args.bind, "gateway listening");
    let connection_permits = Arc::new(Semaphore::new(MAX_AGENT_CONNECTIONS));

    loop {
        let (stream, peer) = listener.accept().await?;
        let Some(permit) =
            try_acquire_agent_connection_permit(&connection_permits, &api_client, peer)
        else {
            continue;
        };
        info!(%peer, "accepted agent connection");
        let args = args.clone();
        let state = state.clone();
        let api_client = api_client.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = handle_agent(stream, peer, args, state, api_client).await {
                warn!(%peer, %error, "agent session ended with error");
            }
        });
    }
}

fn try_acquire_agent_connection_permit(
    connection_permits: &Arc<Semaphore>,
    api_client: &GatewayControlClient,
    peer: SocketAddr,
) -> Option<OwnedSemaphorePermit> {
    match connection_permits.clone().try_acquire_owned() {
        Ok(permit) => Some(permit),
        Err(_) => {
            api_client
                .forward_metrics()
                .record_rejected_agent_connection();
            warn!(
                %peer,
                max_agent_connections = MAX_AGENT_CONNECTIONS,
                "rejected agent connection because gateway admission is full"
            );
            None
        }
    }
}

async fn handle_agent(
    stream: TcpStream,
    peer: SocketAddr,
    args: Args,
    state: GatewayState,
    control: GatewayControlClient,
) -> Result<()> {
    let mut stream = accept_noise_stream(stream, &args).await?;
    let noise_public_key_hex = stream.remote_static().map(hex::encode);
    let remote_ip = peer.ip().to_string();
    let session_id = uuid::Uuid::new_v4();
    let mut client_id = None::<String>;
    let mut process_incarnation_id = None::<uuid::Uuid>;
    let (command_tx, mut command_rx) =
        mpsc::channel::<GatewaySessionMessage>(SESSION_COMMAND_QUEUE_CAPACITY);
    let (close_tx, mut close_rx) = watch::channel(None::<String>);
    let mut outbound_seq = 2_u64;
    let mut pending_commands = HashMap::<uuid::Uuid, PendingCommand>::new();
    let mut pending_cancels = HashMap::new();

    let result: Result<()> = loop {
        tokio::select! {
            biased;
            changed = close_rx.changed(), if client_id.is_some() => {
                if changed.is_err() {
                    break Err(anyhow!("agent_session_close_channel_dropped"));
                }
                let reason = close_rx
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| "gateway_requested_disconnect".to_string());
                break Err(anyhow!("agent_session_closed_by_gateway:{reason}"));
            }
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
                    close_tx: &close_tx,
                };
                if let Err(error) =
                    handle_agent_frame(
                        &mut stream,
                        context,
                        &mut client_id,
                        &mut process_incarnation_id,
                        &mut pending_commands,
                        &mut pending_cancels,
                        frame,
                    ).await
                {
                    break Err(error);
                }
            }
            message = command_rx.recv(), if client_id.is_some() => {
                let Some(message) = message else {
                    continue;
                };
                let client_id = client_id.clone().unwrap_or_default();
                match message {
                    GatewaySessionMessage::Command(command) => {
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
                        pending_commands.insert(job_id, PendingCommand {
                            client_id,
                            job_id,
                            command_version,
                            payload_hash: command.payload_hash.clone(),
                            ack: None,
                            outputs: Vec::new(),
                            response: Some(command.response),
                        });
                    }
                    GatewaySessionMessage::Cancel(cancel) => {
                        let job_id = cancel.request.job_id;
                        if let Err(error) = write_json_frame(
                            &mut stream,
                            MessageKind::CommandCancel,
                            1,
                            outbound_seq,
                            &cancel.request,
                        )
                        .await
                        {
                            break Err(error);
                        }
                        outbound_seq += 1;
                        pending_cancels.insert(job_id, cancel.response);
                    }
                }
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
        let target_key = end_event.client_id.clone();
        control
            .post(
                &target_key,
                "/internal/v1/gateway/session-ended",
                &end_event,
            )
            .await
            .unwrap_or_else(|error| {
                warn!(
                    %error,
                    client_id = %target_key,
                    "failed to enqueue gateway session-ended event"
                )
            });
    }
    result
}

async fn request_agent_disconnect(state: &GatewayState, client_id: &str, reason: &str) {
    if !close_agent_session_now(state, client_id, reason).await {
        warn!(
            client_id,
            reason, "critical gateway forwarding failure had no active agent session to close"
        );
    }
}

async fn request_all_agent_disconnects(state: &GatewayState, reason: &str) {
    let client_ids = state
        .sessions
        .read()
        .await
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for client_id in client_ids {
        request_agent_disconnect(state, &client_id, reason).await;
    }
}

async fn register_session(state: &GatewayState, client_id: &str, session: GatewaySession) {
    let previous = state
        .sessions
        .write()
        .await
        .insert(client_id.to_string(), session);
    if let Some(previous) = previous {
        let _ = previous
            .close_tx
            .send(Some("replaced_by_new_session".to_string()));
    }
}

async fn close_agent_session_now(state: &GatewayState, client_id: &str, reason: &str) -> bool {
    let previous = state.sessions.write().await.remove(client_id);
    let Some(session) = previous else {
        return false;
    };
    let _ = session.close_tx.send(Some(reason.to_string()));
    true
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

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = async {
                if let Some(signal) = terminate.as_mut() {
                    signal.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

struct AgentFrameContext<'a> {
    args: &'a Args,
    state: &'a GatewayState,
    control: &'a GatewayControlClient,
    noise_public_key_hex: Option<String>,
    remote_ip: &'a str,
    session_id: uuid::Uuid,
    command_tx: &'a mpsc::Sender<GatewaySessionMessage>,
    close_tx: &'a watch::Sender<Option<String>>,
}

async fn handle_agent_frame(
    stream: &mut NoiseFrameStream<TcpStream>,
    context: AgentFrameContext<'_>,
    client_id: &mut Option<String>,
    process_incarnation_id: &mut Option<uuid::Uuid>,
    pending_commands: &mut HashMap<uuid::Uuid, PendingCommand>,
    pending_cancels: &mut HashMap<
        uuid::Uuid,
        tokio::sync::oneshot::Sender<vpsman_common::GatewayCommandCancelResult>,
    >,
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
                gateway_session_id: context.session_id,
                noise_public_key_hex: context.noise_public_key_hex.clone(),
                remote_ip: Some(context.remote_ip.to_string()),
                hello: hello.clone(),
            };
            let acceptance = context.control.accept_agent_session(&ingest).await?;
            if !acceptance.accepted {
                anyhow::bail!(
                    "agent session rejected for {}: {}",
                    hello.client_id,
                    acceptance.message
                );
            }
            *client_id = Some(hello.client_id.clone());
            *process_incarnation_id = Some(hello.process_incarnation_id);
            register_session(
                context.state,
                &hello.client_id,
                GatewaySession {
                    session_id: context.session_id,
                    process_incarnation_id: hello.process_incarnation_id,
                    sender: context.command_tx.clone(),
                    close_tx: context.close_tx.clone(),
                },
            )
            .await;
            context
                .state
                .disconnected_at
                .write()
                .await
                .remove(&hello.client_id);
            let reply = ServerHello {
                server_id: context.args.gateway_id.clone(),
                server_version: crate::build_info::release_version().to_string(),
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
            validate_telemetry_session_client_id(client_id.as_deref(), &telemetry.client_id)?;
            let active_process_incarnation_id = process_incarnation_id
                .as_ref()
                .copied()
                .context("telemetry_before_hello")?;
            info!(
                client_id = %telemetry.client_id,
                hostname = %telemetry.metrics.hostname,
                uptime = telemetry.metrics.uptime_secs,
                "telemetry received"
            );
            let ingest = GatewayTelemetryIngest {
                gateway_id: context.args.gateway_id.clone(),
                gateway_session_id: context.session_id,
                process_incarnation_id: active_process_incarnation_id,
                remote_ip: Some(context.remote_ip.to_string()),
                telemetry,
            };
            let target_key = ingest.telemetry.client_id.clone();
            context
                .control
                .post(&target_key, "/internal/v1/gateway/telemetry", &ingest)
                .await?;
        }
        MessageKind::CommandAck => {
            let ack: JobAck = decode_json(&frame.decoded_payload()?)?;
            let mut remove_job_id = None;
            if let Some(pending) = pending_commands.get_mut(&ack.job_id) {
                if !ack.accepted {
                    remove_job_id = Some(pending.job_id);
                    finish_pending_command_response(pending, Some(ack), Vec::new());
                } else {
                    pending.ack = Some(ack.clone());
                    finish_pending_command_response(pending, Some(ack), Vec::new());
                }
            }
            if let Some(job_id) = remove_job_id {
                pending_commands.remove(&job_id);
            }
        }
        MessageKind::CommandCancelAck => {
            let ack: JobCancelAck = decode_json(&frame.decoded_payload()?)?;
            if let Some(response) = pending_cancels.remove(&ack.job_id) {
                let client_id = client_id.clone().unwrap_or_default();
                let _ = response.send(cancel_ack_result(client_id, ack));
            }
        }
        MessageKind::CommandResume => {
            let resume: CommandResume = decode_json(&frame.decoded_payload()?)?;
            let Some(active_client_id) = client_id.clone() else {
                return Ok(());
            };
            pending_commands
                .entry(resume.job_id)
                .and_modify(|pending| {
                    pending.command_version = resume.command_version;
                    pending.client_id = active_client_id.clone();
                    pending.payload_hash = resume.payload_hash.clone();
                })
                .or_insert_with(|| PendingCommand {
                    client_id: active_client_id,
                    job_id: resume.job_id,
                    command_version: resume.command_version,
                    payload_hash: resume.payload_hash.clone(),
                    ack: Some(JobAck {
                        job_id: resume.job_id,
                        accepted: true,
                        message: "resumed".to_string(),
                    }),
                    outputs: Vec::new(),
                    response: None,
                });
            debug!(
                job_id = %resume.job_id,
                payload_hash = %resume.payload_hash,
                next_output_seq = resume.next_output_seq,
                "resumed active agent command"
            );
        }
        MessageKind::CommandOutput => {
            let sequenced: SequencedCommandOutput = decode_json(&frame.decoded_payload()?)?;
            let output = sequenced.output;
            let mut remove_job_id = None;
            if let Some(pending) = pending_commands.get_mut(&output.job_id) {
                let done = output.done;
                let active_process_incarnation_id = process_incarnation_id
                    .as_ref()
                    .copied()
                    .context("command_output_before_hello")?;
                let ingest = GatewayCommandOutputIngest {
                    gateway_id: context.args.gateway_id.clone(),
                    gateway_session_id: context.session_id,
                    process_incarnation_id: active_process_incarnation_id,
                    client_id: pending.client_id.clone(),
                    job_id: output.job_id,
                    payload_hash: pending.payload_hash.clone(),
                    seq: sequenced.seq,
                    received_unix: Some(unix_now()),
                    output: output.clone(),
                };
                context
                    .control
                    .post_command_output(&pending.client_id, &ingest)
                    .await?;
                let truncated = pending.retain_output_if_response_waiting(output);
                if truncated > 0 {
                    context
                        .state
                        .forward_metrics
                        .record_retained_output_truncated(truncated);
                }
                if done {
                    remove_job_id = Some(pending.job_id);
                }
            }
            if let Some(job_id) = remove_job_id {
                pending_commands.remove(&job_id);
            }
        }
        MessageKind::TerminalStreamOutput => {
            let output: vpsman_common::TerminalStreamOutput =
                decode_json(&frame.decoded_payload()?)?;
            let Some(client_id) = client_id.clone() else {
                return Ok(());
            };
            let active_process_incarnation_id = process_incarnation_id
                .as_ref()
                .copied()
                .context("terminal_output_before_hello")?;
            let target_key = client_id.clone();
            let ingest = GatewayTerminalOutputIngest {
                gateway_id: context.args.gateway_id.clone(),
                gateway_session_id: context.session_id,
                process_incarnation_id: active_process_incarnation_id,
                client_id,
                output,
            };
            context
                .control
                .post(&target_key, "/internal/v1/gateway/terminal-output", &ingest)
                .await?;
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

fn validate_telemetry_session_client_id(
    authenticated_client_id: Option<&str>,
    telemetry_client_id: &str,
) -> Result<()> {
    let Some(authenticated_client_id) = authenticated_client_id else {
        anyhow::bail!("telemetry_before_hello");
    };
    if telemetry_client_id != authenticated_client_id {
        anyhow::bail!("telemetry_client_id_mismatch");
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
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
        let (older_tx, _older_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
        let (newer_tx, _newer_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
        let (older_close_tx, _older_close_rx) = watch::channel(None::<String>);
        let (newer_close_tx, _newer_close_rx) = watch::channel(None::<String>);
        state.sessions.write().await.insert(
            "client-a".to_string(),
            GatewaySession {
                session_id: older_session_id,
                process_incarnation_id: uuid::Uuid::new_v4(),
                sender: older_tx,
                close_tx: older_close_tx,
            },
        );
        let newer_process_incarnation_id = uuid::Uuid::new_v4();
        state.sessions.write().await.insert(
            "client-a".to_string(),
            GatewaySession {
                session_id: newer_session_id,
                process_incarnation_id: newer_process_incarnation_id,
                sender: newer_tx,
                close_tx: newer_close_tx,
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

    #[tokio::test]
    async fn registering_replacement_session_closes_displaced_session() {
        let state = GatewayState::default();
        let (older_tx, _older_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
        let (newer_tx, _newer_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
        let (older_close_tx, mut older_close_rx) = watch::channel(None::<String>);
        let (newer_close_tx, _newer_close_rx) = watch::channel(None::<String>);
        let newer_session_id = uuid::Uuid::new_v4();

        register_session(
            &state,
            "client-a",
            GatewaySession {
                session_id: uuid::Uuid::new_v4(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                sender: older_tx,
                close_tx: older_close_tx,
            },
        )
        .await;
        register_session(
            &state,
            "client-a",
            GatewaySession {
                session_id: newer_session_id,
                process_incarnation_id: uuid::Uuid::new_v4(),
                sender: newer_tx,
                close_tx: newer_close_tx,
            },
        )
        .await;

        older_close_rx.changed().await.unwrap();
        assert_eq!(
            older_close_rx.borrow().as_deref(),
            Some("replaced_by_new_session")
        );
        assert_eq!(
            state
                .sessions
                .read()
                .await
                .get("client-a")
                .map(|session| session.session_id),
            Some(newer_session_id)
        );
    }

    #[test]
    fn internal_token_startup_validation_rejects_missing_short_or_placeholder() {
        assert!(required_internal_token(None).is_err());
        assert!(required_internal_token(Some("short")).is_err());
        assert!(required_internal_token(Some("change-me-internal-token")).is_err());
        assert!(required_internal_token(Some("dev-internal-token-change-me-32chars")).is_err());
        assert!(
            required_internal_token(Some("replace-with-random-token-at-least-32-chars")).is_err()
        );
        assert!(required_internal_token(Some("real-internal-token-value-32-plus-chars")).is_ok());
    }

    #[test]
    fn gateway_bind_defaults_to_loopback() {
        with_cleared_gateway_env(&["VPSMAN_GATEWAY_BIND"], || {
            let args = Args::parse_from(["vpsman-gateway"]);
            assert_eq!(args.bind, "127.0.0.1:9443");
        });
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
        args.api_url = None;
        assert!(validate_gateway_runtime_mode(&args)
            .unwrap_err()
            .to_string()
            .contains("VPSMAN_API_URL is required"));

        args.api_url = Some("http://127.0.0.1:8080".to_string());
        validate_gateway_runtime_mode(&args).unwrap();
    }

    #[test]
    fn agent_connection_admission_records_rejection_when_full() {
        let permits = Arc::new(Semaphore::new(0));
        let client = GatewayControlClient::new(
            Some("http://127.0.0.1:8080".to_string()),
            None,
            GatewayHttpTimeouts::default(),
        );
        let peer = "127.0.0.1:10000".parse().unwrap();

        assert!(try_acquire_agent_connection_permit(&permits, &client, peer).is_none());
        assert_eq!(
            client
                .forward_metrics()
                .snapshot()
                .rejected_agent_connections,
            1
        );
    }

    #[test]
    fn telemetry_client_id_must_match_authenticated_session() {
        validate_telemetry_session_client_id(Some("client-a"), "client-a").unwrap();
        assert_eq!(
            validate_telemetry_session_client_id(None, "client-a")
                .unwrap_err()
                .to_string(),
            "telemetry_before_hello"
        );
        assert_eq!(
            validate_telemetry_session_client_id(Some("client-a"), "client-b")
                .unwrap_err()
                .to_string(),
            "telemetry_client_id_mismatch"
        );
    }

    #[test]
    fn gateway_runtime_config_reloads_suite_file_from_base_args() {
        with_cleared_gateway_env(GATEWAY_HOT_RELOAD_ENV, || {
            let path = temp_suite_config_path("gateway-hot-reload");
            std::fs::write(&path, gateway_runtime_toml(45, 31, 4, 5, 6, 7, 900)).unwrap();
            let mut args = test_args();
            args.suite_config = path.clone();

            let runtime = load_gateway_runtime_config(&args).unwrap();

            assert_eq!(runtime.reconnect_grace_secs, 45);
            assert_eq!(runtime.dispatch_ack_secs, 31);
            assert_eq!(runtime.http_timeouts.connect.as_secs(), 4);
            assert_eq!(runtime.http_timeouts.write.as_secs(), 5);
            assert_eq!(runtime.http_timeouts.read.as_secs(), 6);
            assert_eq!(runtime.http_timeouts.event_post.as_secs(), 7);
            assert_eq!(runtime.forward_config.command_output_event_ttl_secs, 900);

            std::fs::write(&path, gateway_runtime_toml(75, 41, 8, 9, 10, 11, 1800)).unwrap();

            let runtime = load_gateway_runtime_config(&args).unwrap();
            assert_eq!(runtime.reconnect_grace_secs, 75);
            assert_eq!(runtime.dispatch_ack_secs, 41);
            assert_eq!(runtime.http_timeouts.connect.as_secs(), 8);
            assert_eq!(runtime.http_timeouts.write.as_secs(), 9);
            assert_eq!(runtime.http_timeouts.read.as_secs(), 10);
            assert_eq!(runtime.http_timeouts.event_post.as_secs(), 11);
            assert_eq!(runtime.forward_config.command_output_event_ttl_secs, 1800);

            let _ = std::fs::remove_file(path);
        });
    }

    fn test_args() -> Args {
        Args {
            bind: "127.0.0.1:0".to_string(),
            control_bind: "127.0.0.1:0".to_string(),
            suite_config: std::path::PathBuf::from("config/vpsman.toml"),
            private_key_hex: Some("11".repeat(32)),
            expect_client_public_key_hex: None,
            api_url: Some("http://127.0.0.1:8080".to_string()),
            internal_token: Some("real-internal-token-value-32-plus-chars".to_string()),
            privilege_verifier_key_hex: Some("11".repeat(32)),
            gateway_id: "test-gateway".to_string(),
            reconnect_grace_secs: 60,
            internal_http_connect_secs: 10,
            internal_http_write_secs: 10,
            internal_http_read_secs: 15,
            event_post_secs: 15,
            dispatch_ack_secs: 30,
            spool_dir: std::path::PathBuf::from("./runtime/gateway-spool"),
            spool_ram_max_bytes: 1024 * 1024 * 1024,
            spool_disk_max_bytes: 4 * 1024 * 1024 * 1024,
            spool_shutdown_flush_secs: 30,
            command_output_event_ttl_secs: DEFAULT_COMMAND_OUTPUT_EVENT_TTL_SECS,
        }
    }

    const GATEWAY_HOT_RELOAD_ENV: &[&str] = &[
        "VPSMAN_GATEWAY_RECONNECT_GRACE_SECS",
        "VPSMAN_INTERNAL_HTTP_CONNECT_SECS",
        "VPSMAN_INTERNAL_HTTP_WRITE_SECS",
        "VPSMAN_INTERNAL_HTTP_READ_SECS",
        "VPSMAN_EVENT_POST_SECS",
        "VPSMAN_DISPATCH_ACK_SECS",
        "VPSMAN_GATEWAY_SPOOL_DIR",
        "VPSMAN_GATEWAY_SPOOL_RAM_MAX_BYTES",
        "VPSMAN_GATEWAY_SPOOL_DISK_MAX_BYTES",
        "VPSMAN_GATEWAY_SPOOL_SHUTDOWN_FLUSH_SECS",
        "VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS",
    ];

    static GATEWAY_SUITE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_cleared_gateway_env<R>(names: &[&str], run: impl FnOnce() -> R) -> R {
        let _guard = GATEWAY_SUITE_ENV_LOCK.lock().unwrap();
        let saved = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect::<Vec<_>>();
        for name in names {
            std::env::remove_var(name);
        }
        let result = run();
        for (name, value) in saved {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
        result
    }

    fn temp_suite_config_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("vpsman-{label}-{}.toml", uuid::Uuid::new_v4()))
    }

    fn gateway_runtime_toml(
        reconnect_grace_secs: u64,
        dispatch_ack_secs: u64,
        connect_secs: u64,
        write_secs: u64,
        read_secs: u64,
        event_post_secs: u64,
        command_output_event_ttl_secs: u64,
    ) -> String {
        format!(
            r#"version = 1

[gateway]
reconnect_grace_secs = {reconnect_grace_secs}
command_output_event_ttl_secs = {command_output_event_ttl_secs}

[timeout]
dispatch_ack_secs = {dispatch_ack_secs}
internal_http_connect_secs = {connect_secs}
internal_http_write_secs = {write_secs}
internal_http_read_secs = {read_secs}
event_post_secs = {event_post_secs}
"#
        )
    }
}
