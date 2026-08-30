use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex, OnceLock,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use base64::Engine;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, oneshot, Mutex, Notify, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore},
    time,
};
use tracing::warn;
use vpsman_common::{
    AgentConfig, AgentExecutionEnvironmentPolicy, AgentExecutionPtyPolicy, CommandOutput,
    JobCommand, OutputStream, TerminalControlAck, TerminalControlAction, TerminalControlRequest,
    TerminalStreamOutput, TerminalUserPolicy, MAX_TERMINAL_COLS, MAX_TERMINAL_INPUT_BYTES,
    MAX_TERMINAL_REASON_BYTES, MAX_TERMINAL_ROWS, MIN_TERMINAL_COLS, MIN_TERMINAL_ROWS,
};

use crate::{
    child_process,
    platform_accounts::{current_effective_uid, AccountIdentity, PlatformAccounts},
    process_cleanup::{terminate_process_group_blocking, ProcessCleanupReport},
    telemetry::unix_now,
};

const MAX_TERMINAL_SESSIONS: usize = 8;
const TERMINAL_READ_CHUNK_BYTES: usize = 8192;
const TERMINAL_OUTPUT_SETTLE_MS: u64 = 80;
const TERMINAL_IDLE_SCAN_SECS: u64 = 30;
const TERMINAL_CLOSE_GRACE_MS: u64 = 500;
const TERMINAL_FINAL_EVENT_SEND_TIMEOUT_SECS: u64 = 5;
const TERMINAL_DISCONNECTED_GRACE_SECS: u64 = 3600;

static TERMINAL_REGISTRY: OnceLock<TerminalRegistry> = OnceLock::new();
static TERMINAL_OPEN_OWNERS: OnceLock<StdMutex<HashMap<uuid::Uuid, Arc<Mutex<()>>>>> =
    OnceLock::new();
static TERMINAL_SESSION_CAPACITY: OnceLock<Arc<Semaphore>> = OnceLock::new();
static TERMINAL_PENDING_FINAL_EVENTS: OnceLock<Mutex<TerminalPendingFinalEvents>> = OnceLock::new();
static TERMINAL_PENDING_FINAL_NOTIFY: OnceLock<Notify> = OnceLock::new();

struct TerminalOpenOwner {
    session_id: uuid::Uuid,
    entry: Arc<Mutex<()>>,
    guard: Option<OwnedMutexGuard<()>>,
}

impl Drop for TerminalOpenOwner {
    fn drop(&mut self) {
        self.guard.take();
        let mut owners = terminal_open_owners()
            .lock()
            .expect("terminal-open owner registry poisoned");
        let is_current = owners
            .get(&self.session_id)
            .is_some_and(|entry| Arc::ptr_eq(entry, &self.entry));
        if is_current && Arc::strong_count(&self.entry) == 2 {
            owners.remove(&self.session_id);
        }
    }
}

#[derive(Clone)]
pub(crate) struct PendingTerminalFinalEvent {
    generation: u64,
    pub(crate) event: TerminalStreamOutput,
}

#[derive(Default)]
struct TerminalPendingFinalEvents {
    next_generation: u64,
    events: HashMap<uuid::Uuid, PendingTerminalFinalEvent>,
}

pub(crate) async fn execute_terminal_command(
    config: &AgentConfig,
    job_id: uuid::Uuid,
    command: &JobCommand,
    max_timeout_secs: u64,
) -> Result<Vec<CommandOutput>> {
    execute_terminal_command_with_stream_sink(config, job_id, command, max_timeout_secs, None).await
}

pub(crate) async fn execute_terminal_command_with_stream_sink(
    config: &AgentConfig,
    job_id: uuid::Uuid,
    command: &JobCommand,
    max_timeout_secs: u64,
    stream_tx: Option<mpsc::Sender<TerminalStreamOutput>>,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(max_timeout_secs.max(1)),
        execute_terminal_command_inner(config, job_id, command, stream_tx),
    )
    .await
    .context("terminal command timed out")?
}

pub(crate) async fn mark_gateway_connected() {
    registry().mark_connected().await;
}

pub(crate) async fn mark_gateway_disconnected() {
    registry().mark_disconnected().await;
}

pub(crate) async fn close_all_terminal_sessions_for_lifecycle(reason: &str) {
    let entries = registry().remove_all().await;
    for entry in entries {
        close_removed_terminal_entry(entry, "terminal_stream", "lifecycle_disconnected", reason)
            .await;
    }
}

pub(crate) async fn pending_terminal_final_events() -> Vec<PendingTerminalFinalEvent> {
    let pending = pending_final_events().lock().await;
    let mut events = pending.events.values().cloned().collect::<Vec<_>>();
    events.sort_by_key(|event| event.generation);
    events
}

pub(crate) async fn pending_terminal_final_event_ready() {
    loop {
        let notified = pending_final_notify().notified();
        if !pending_final_events().lock().await.events.is_empty() {
            return;
        }
        notified.await;
    }
}

pub(crate) async fn acknowledge_pending_terminal_final_event(
    delivered: &PendingTerminalFinalEvent,
) {
    let mut pending = pending_final_events().lock().await;
    if pending
        .events
        .get(&delivered.event.session_id)
        .is_some_and(|current| current.generation == delivered.generation)
    {
        pending.events.remove(&delivered.event.session_id);
    }
}

pub(crate) async fn retain_pending_terminal_final_event(event: TerminalStreamOutput) {
    debug_assert!(event.output.done, "only final terminal events are retained");
    let mut pending = pending_final_events().lock().await;
    if pending
        .events
        .get(&event.session_id)
        .is_some_and(|current| {
            event.output_next_seq < current.event.output_next_seq
                || (event.output_next_seq == current.event.output_next_seq
                    && (!event.output.done || !current.event.output.done))
        })
    {
        return;
    }
    pending.next_generation = pending.next_generation.saturating_add(1);
    let generation = pending.next_generation;
    pending.events.insert(
        event.session_id,
        PendingTerminalFinalEvent { generation, event },
    );
    drop(pending);
    pending_final_notify().notify_one();
}

async fn execute_terminal_command_inner(
    config: &AgentConfig,
    job_id: uuid::Uuid,
    command: &JobCommand,
    stream_tx: Option<mpsc::Sender<TerminalStreamOutput>>,
) -> Result<Vec<CommandOutput>> {
    match command {
        JobCommand::TerminalOpen {
            session_id,
            argv,
            cwd,
            user,
            user_policy,
            cols,
            rows,
            replay_from_seq,
            idle_timeout_secs,
            flow_window_bytes,
        } => {
            open_terminal_session(TerminalOpenInput {
                config,
                job_id,
                session_id: *session_id,
                argv,
                cwd: cwd.as_deref(),
                user: user.as_deref(),
                user_policy: *user_policy,
                cols: *cols,
                rows: *rows,
                replay_from_seq: *replay_from_seq,
                idle_timeout_secs: *idle_timeout_secs,
                flow_window_bytes: *flow_window_bytes,
                stream_tx,
            })
            .await
        }
        _ => anyhow::bail!("not a terminal command"),
    }
}

struct TerminalOpenInput<'a> {
    config: &'a AgentConfig,
    job_id: uuid::Uuid,
    session_id: uuid::Uuid,
    argv: &'a [String],
    cwd: Option<&'a str>,
    user: Option<&'a str>,
    user_policy: TerminalUserPolicy,
    cols: u16,
    rows: u16,
    replay_from_seq: Option<u64>,
    idle_timeout_secs: u32,
    flow_window_bytes: u32,
    stream_tx: Option<mpsc::Sender<TerminalStreamOutput>>,
}

async fn open_terminal_session(input: TerminalOpenInput<'_>) -> Result<Vec<CommandOutput>> {
    if input.config.execution.pty_policy == AgentExecutionPtyPolicy::Disabled {
        return Ok(vec![status_output(
            input.job_id,
            serde_json::json!({
                "type": "terminal_open",
                "status": "rejected",
                "reason": "execution_pty_policy_disabled",
                "session_id": input.session_id,
            }),
            Some(126),
        )]);
    }
    validate_terminal_argv(input.argv)?;
    let effective_cwd = input
        .cwd
        .or(input.config.execution.working_directory.as_deref());
    validate_terminal_cwd(effective_cwd)?;
    let user_resolution = resolve_terminal_user(input.user, input.user_policy)?;
    let _open_owner = acquire_terminal_open_owner(input.session_id).await;
    let registry = registry();
    if let Some(handle) = registry.get_handle(input.session_id).await {
        if handle.open_job_id != input.job_id {
            return Ok(vec![status_output(
                input.job_id,
                serde_json::json!({
                    "type": "terminal_open",
                    "status": "rejected",
                    "reason": "terminal_session_id_in_use",
                    "session_id": input.session_id,
                }),
                Some(125),
            )]);
        }
        handle.update_stream_sender(input.stream_tx).await;
        handle.last_activity.store(unix_now(), Ordering::Relaxed);
        let (outputs, range) = collect_session_output(
            input.job_id,
            input.session_id,
            Some(input.replay_from_seq.unwrap_or_default()),
        )
        .await;
        return Ok(with_status(
            outputs,
            input.job_id,
            status_with_output_range(
                serde_json::json!({
                "type": "terminal_open",
                "status": "attached",
                "session_id": input.session_id,
                "session_exited": handle.session_exited().await,
                }),
                &range,
            ),
            Some(0),
        ));
    }
    let capacity_owner = match terminal_session_capacity().try_acquire_owned() {
        Ok(owner) => owner,
        Err(_) => {
            return Ok(vec![status_output(
                input.job_id,
                serde_json::json!({
                    "type": "terminal_open",
                    "status": "rejected",
                    "reason": "terminal_session_limit_reached",
                    "session_id": input.session_id,
                    "max_sessions": MAX_TERMINAL_SESSIONS,
                }),
                Some(125),
            )]);
        }
    };

    let pty = child_process::open_pty_stdio().context("failed to open terminal PTY")?;
    child_process::set_pty_window_size(&pty.master, input.cols, input.rows)
        .context("failed to set terminal PTY window size")?;
    let child_process::PtyStdio {
        master,
        control,
        stdin,
        stdout,
        stderr,
    } = pty;
    let reader = tokio::fs::File::from_std(master.try_clone()?);
    let writer = tokio::fs::File::from_std(master);

    let mut command = tokio::process::Command::new(&input.argv[0]);
    command.args(&input.argv[1..]);
    if let Some(cwd) = effective_cwd {
        command.current_dir(cwd);
    }
    apply_terminal_environment(input.config, &mut command);
    if let Some(identity) = user_resolution.identity.as_ref() {
        command.uid(identity.uid);
        command.gid(identity.gid);
    }
    command.kill_on_drop(true);
    child_process::configure_controlling_pty(&mut command, &control);
    command.stdin(stdin);
    command.stdout(stdout);
    command.stderr(stderr);

    let child = command
        .spawn()
        .context("failed to spawn terminal command")?;
    drop(control);
    let process_group_id = child
        .id()
        .map(|pid| pid as libc::pid_t)
        .context("terminal child process id unavailable")?;
    let handle = TerminalSessionHandle {
        session_id: input.session_id,
        open_job_id: input.job_id,
        writer: Arc::new(Mutex::new(writer)),
        output: Arc::new(Mutex::new(TerminalOutputBuffer::new(
            input.flow_window_bytes as usize,
        ))),
        exit_code: Arc::new(Mutex::new(None)),
        process_group_id,
        last_activity: Arc::new(AtomicU64::new(unix_now())),
        stream_tx: Arc::new(Mutex::new(input.stream_tx)),
    };
    let (session_start_tx, session_start_rx) = oneshot::channel();
    let session_handle = handle.clone();
    let session_id = input.session_id;
    let open_job_id = input.job_id;
    let idle_timeout_secs = input.idle_timeout_secs;
    let session_registry = registry;
    let session_task = tokio::spawn(async move {
        if session_start_rx.await.is_err() {
            return;
        }
        let mut supervised = TerminalSessionRunOwner(tokio::spawn(run_terminal_session(
            reader,
            child,
            session_handle,
            idle_timeout_secs,
        )));
        let supervised = (&mut supervised.0).await;
        if let Err(error) = supervised {
            warn!(
                %error,
                %session_id,
                %open_job_id,
                "terminal session consumer failed"
            );
            if let Some(mut entry) = session_registry
                .remove_if_current(session_id, open_job_id)
                .await
            {
                if let Some(owner) = entry._session_owner.as_mut() {
                    owner.disarm();
                }
                close_removed_terminal_entry(
                    entry,
                    "terminal_stream",
                    "failed",
                    "terminal_session_consumer_failed",
                )
                .await;
            }
        }
    });
    registry
        .insert(
            input.session_id,
            TerminalRegistryEntry {
                handle: handle.clone(),
                last_delivered_seq: input.replay_from_seq.unwrap_or_default(),
                last_input_seq: 0,
                disconnected_since: None,
                idle_timeout_secs: input.idle_timeout_secs,
                cols: input.cols,
                rows: input.rows,
                _capacity_owner: Some(capacity_owner),
                _session_owner: Some(TerminalSessionOwner(Some(session_task.abort_handle()))),
            },
        )
        .await;
    if session_start_tx.send(()).is_err() {
        let _ = registry
            .remove_if_current(input.session_id, input.job_id)
            .await;
        anyhow::bail!("terminal session consumer stopped before ownership handoff");
    }

    time::sleep(Duration::from_millis(TERMINAL_OUTPUT_SETTLE_MS)).await;
    let (outputs, range) = collect_session_output(
        input.job_id,
        input.session_id,
        Some(input.replay_from_seq.unwrap_or_default()),
    )
    .await;
    Ok(with_status(
        outputs,
        input.job_id,
        status_with_output_range(
            serde_json::json!({
            "type": "terminal_open",
            "status": "opened",
            "session_id": input.session_id,
            "argv": input.argv,
            "cwd": effective_cwd,
            "requested_user": input.user,
            "user_policy": input.user_policy,
            "user_resolution": user_resolution.status,
            "resolved_uid": user_resolution.identity.as_ref().map(|identity| identity.uid),
            "resolved_gid": user_resolution.identity.as_ref().map(|identity| identity.gid),
            "environment_policy": input.config.execution.environment_policy,
            "pty_policy": input.config.execution.pty_policy,
            "cols": input.cols,
            "rows": input.rows,
            "idle_timeout_secs": input.idle_timeout_secs,
            "flow_window_bytes": input.flow_window_bytes,
            "session_exited": handle.session_exited().await,
            }),
            &range,
        ),
        Some(0),
    ))
}

struct TerminalUserResolution {
    identity: Option<AccountIdentity>,
    status: &'static str,
}

fn resolve_terminal_user(
    user: Option<&str>,
    policy: TerminalUserPolicy,
) -> Result<TerminalUserResolution> {
    let Some(user) = user.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(TerminalUserResolution {
            identity: None,
            status: "agent_user",
        });
    };
    let accounts = PlatformAccounts::load()?;
    let Some(identity) = accounts.find_user_identity(user) else {
        return terminal_user_unavailable(policy, "requested_terminal_user_not_found");
    };
    let current_uid = current_effective_uid();
    if current_uid == identity.uid {
        return Ok(TerminalUserResolution {
            identity: None,
            status: "requested_user_already_effective",
        });
    }
    if current_uid != 0 {
        return terminal_user_unavailable(policy, "agent_not_root_for_terminal_user_switch");
    }
    Ok(TerminalUserResolution {
        identity: Some(identity),
        status: "requested_user",
    })
}

fn terminal_user_unavailable(
    policy: TerminalUserPolicy,
    reason: &'static str,
) -> Result<TerminalUserResolution> {
    match policy {
        TerminalUserPolicy::Fail => anyhow::bail!(reason),
        TerminalUserPolicy::Fallback => Ok(TerminalUserResolution {
            identity: None,
            status: reason,
        }),
    }
}

fn apply_terminal_environment(config: &AgentConfig, command: &mut tokio::process::Command) {
    match config.execution.environment_policy {
        AgentExecutionEnvironmentPolicy::Inherit => {}
        AgentExecutionEnvironmentPolicy::Clean => {
            command.env_clear();
            apply_kept_environment(config, command);
        }
        AgentExecutionEnvironmentPolicy::MinimalPath => {
            command.env_clear();
            command.env(
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            );
            apply_kept_environment(config, command);
        }
    }
    for (key, value) in &config.execution.environment_set {
        command.env(key, value);
    }
}

fn apply_kept_environment(config: &AgentConfig, command: &mut tokio::process::Command) {
    for key in &config.execution.environment_keep {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

pub(crate) async fn control_terminal_session(
    request: TerminalControlRequest,
) -> TerminalControlAck {
    let action = request.action.kind().to_string();
    let rejected = |status: &str, message: &str| TerminalControlAck {
        request_id: request.request_id,
        session_id: request.session_id,
        action: action.clone(),
        accepted: false,
        status: status.to_string(),
        message: message.to_string(),
        input_seq: None,
        written_bytes: None,
        cols: None,
        rows: None,
    };
    if request.request_id.is_nil() || request.session_id.is_nil() {
        return rejected("rejected", "terminal_control_id_invalid");
    }
    match request.action {
        TerminalControlAction::Input { data_base64 } => {
            let data = match base64::engine::general_purpose::STANDARD.decode(data_base64) {
                Ok(data) if !data.is_empty() && data.len() <= MAX_TERMINAL_INPUT_BYTES => data,
                _ => {
                    return rejected("rejected", "terminal_input_size_or_encoding_invalid");
                }
            };
            let Some((handle, input_seq)) = registry().next_input(request.session_id).await else {
                return rejected("missing", "terminal_session_not_open");
            };
            if handle.session_exited().await {
                return rejected("exited", "terminal_session_exited");
            }
            let write_result = async {
                let mut writer = handle.writer.lock().await;
                writer.write_all(&data).await?;
                writer.flush().await
            }
            .await;
            if write_result.is_err() {
                return rejected("failed", "terminal_input_write_failed");
            }
            handle.last_activity.store(unix_now(), Ordering::Relaxed);
            TerminalControlAck {
                request_id: request.request_id,
                session_id: request.session_id,
                action,
                accepted: true,
                status: "accepted".to_string(),
                message: "terminal_input_accepted".to_string(),
                input_seq: Some(input_seq),
                written_bytes: Some(data.len() as u64),
                cols: None,
                rows: None,
            }
        }
        TerminalControlAction::Resize { cols, rows } => {
            if !(MIN_TERMINAL_COLS..=MAX_TERMINAL_COLS).contains(&cols)
                || !(MIN_TERMINAL_ROWS..=MAX_TERMINAL_ROWS).contains(&rows)
            {
                return rejected("rejected", "terminal_dimensions_out_of_range");
            }
            let Some(handle) = registry().resize(request.session_id, cols, rows).await else {
                return rejected("missing", "terminal_session_not_open");
            };
            if handle.session_exited().await {
                return rejected("exited", "terminal_session_exited");
            }
            let writer = handle.writer.lock().await;
            if child_process::set_pty_window_size(&*writer, cols, rows).is_err() {
                return rejected("failed", "terminal_resize_failed");
            }
            handle.last_activity.store(unix_now(), Ordering::Relaxed);
            TerminalControlAck {
                request_id: request.request_id,
                session_id: request.session_id,
                action,
                accepted: true,
                status: "resized".to_string(),
                message: "terminal_resized".to_string(),
                input_seq: None,
                written_bytes: None,
                cols: Some(cols),
                rows: Some(rows),
            }
        }
        TerminalControlAction::Close { reason } => {
            if validate_terminal_reason(reason.as_deref()).is_err() {
                return rejected("rejected", "terminal_close_reason_invalid");
            }
            let Some(entry) = registry().remove(request.session_id).await else {
                return rejected("missing", "terminal_session_not_open");
            };
            if entry.handle.session_exited().await {
                return rejected("exited", "terminal_session_exited");
            }
            close_removed_terminal_entry(
                entry,
                "terminal_close",
                "closed",
                reason.as_deref().unwrap_or("operator"),
            )
            .await;
            TerminalControlAck {
                request_id: request.request_id,
                session_id: request.session_id,
                action,
                accepted: true,
                status: "closed".to_string(),
                message: "terminal_closed".to_string(),
                input_seq: None,
                written_bytes: None,
                cols: None,
                rows: None,
            }
        }
    }
}

#[derive(Clone)]
struct TerminalSessionHandle {
    session_id: uuid::Uuid,
    open_job_id: uuid::Uuid,
    writer: Arc<Mutex<tokio::fs::File>>,
    output: Arc<Mutex<TerminalOutputBuffer>>,
    exit_code: Arc<Mutex<Option<Option<i32>>>>,
    process_group_id: libc::pid_t,
    last_activity: Arc<AtomicU64>,
    stream_tx: Arc<Mutex<Option<mpsc::Sender<TerminalStreamOutput>>>>,
}

impl TerminalSessionHandle {
    async fn session_exited(&self) -> bool {
        self.exit_code.lock().await.is_some()
    }

    async fn update_stream_sender(&self, stream_tx: Option<mpsc::Sender<TerminalStreamOutput>>) {
        if stream_tx.is_some() {
            *self.stream_tx.lock().await = stream_tx;
        }
    }

    async fn has_stream_sender(&self) -> bool {
        self.stream_tx.lock().await.is_some()
    }

    async fn emit_stream_chunk(&self, chunk: TerminalOutputChunk, range: TerminalOutputRange) {
        let output = CommandOutput {
            job_id: self.open_job_id,
            stream: OutputStream::Pty,
            data: chunk.data,
            exit_code: None,
            done: false,
        };
        self.emit_stream_output(Some(chunk.seq), range, output, false)
            .await;
    }

    async fn emit_stream_status(
        &self,
        event_type: &'static str,
        status: &'static str,
        done: bool,
        exit_code: Option<i32>,
    ) {
        self.emit_stream_status_with_reason(event_type, status, done, exit_code, None)
            .await;
    }

    async fn emit_stream_status_with_reason(
        &self,
        event_type: &'static str,
        status: &'static str,
        done: bool,
        exit_code: Option<i32>,
        reason: Option<&str>,
    ) {
        let range = self.output.lock().await.range_from(1);
        let mut status_value = serde_json::json!({
            "type": event_type,
            "status": status,
            "session_id": self.session_id,
            "session_exited": self.session_exited().await,
        });
        if let Some(reason) = reason {
            if let Some(object) = status_value.as_object_mut() {
                object.insert("reason".to_string(), serde_json::json!(reason));
            }
        }
        let status = status_with_output_range(status_value, &range);
        let output = CommandOutput {
            job_id: self.open_job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status).unwrap_or_default(),
            exit_code,
            done,
        };
        self.emit_stream_output(None, range, output, done).await;
    }

    async fn emit_stream_output(
        &self,
        terminal_seq: Option<u64>,
        range: TerminalOutputRange,
        output: CommandOutput,
        reliable: bool,
    ) {
        let event = TerminalStreamOutput {
            job_id: self.open_job_id,
            session_id: self.session_id,
            terminal_seq,
            output_first_seq: range.first_seq,
            output_next_seq: range.next_seq,
            output_retained_first_seq: range.retained_first_seq,
            output_retained_bytes: range.retained_bytes as u64,
            output_dropped_bytes: range.dropped_bytes,
            output_dropped_chunks: range.dropped_chunks,
            output_replay_truncated: range.replay_truncated,
            output,
        };
        let Some(stream_tx) = self.stream_tx.lock().await.clone() else {
            return;
        };
        if reliable {
            match time::timeout(
                Duration::from_secs(TERMINAL_FINAL_EVENT_SEND_TIMEOUT_SECS),
                stream_tx.send(event.clone()),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    retain_pending_terminal_final_event(event).await;
                    warn!(
                        session_id = %self.session_id,
                        "terminal final stream status could not be queued because the stream receiver closed"
                    );
                }
                Err(_) => {
                    retain_pending_terminal_final_event(event).await;
                    warn!(
                        session_id = %self.session_id,
                        "terminal final stream status timed out while waiting for queue capacity"
                    );
                }
            }
        } else {
            let _ = stream_tx.try_send(event);
        }
    }
}

struct TerminalRegistry {
    sessions: Mutex<HashMap<uuid::Uuid, TerminalRegistryEntry>>,
}

impl TerminalRegistry {
    #[cfg(test)]
    async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    async fn get_handle(&self, session_id: uuid::Uuid) -> Option<TerminalSessionHandle> {
        self.sessions
            .lock()
            .await
            .get(&session_id)
            .map(|entry| entry.handle.clone())
    }

    async fn collect_handle(
        &self,
        session_id: uuid::Uuid,
        from_seq: Option<u64>,
    ) -> Option<(TerminalSessionHandle, u64)> {
        self.sessions.lock().await.get(&session_id).map(|entry| {
            (
                entry.handle.clone(),
                from_seq.unwrap_or(entry.last_delivered_seq),
            )
        })
    }

    async fn insert(&self, session_id: uuid::Uuid, entry: TerminalRegistryEntry) {
        self.sessions.lock().await.insert(session_id, entry);
    }

    async fn remove(&self, session_id: uuid::Uuid) -> Option<TerminalRegistryEntry> {
        self.sessions.lock().await.remove(&session_id)
    }

    async fn remove_if_current(
        &self,
        session_id: uuid::Uuid,
        open_job_id: uuid::Uuid,
    ) -> Option<TerminalRegistryEntry> {
        let mut sessions = self.sessions.lock().await;
        if !sessions
            .get(&session_id)
            .is_some_and(|entry| entry.handle.open_job_id == open_job_id)
        {
            return None;
        }
        sessions.remove(&session_id)
    }

    async fn next_input(&self, session_id: uuid::Uuid) -> Option<(TerminalSessionHandle, u64)> {
        let mut sessions = self.sessions.lock().await;
        let entry = sessions.get_mut(&session_id)?;
        entry.disconnected_since = None;
        entry.last_input_seq = entry.last_input_seq.saturating_add(1);
        Some((entry.handle.clone(), entry.last_input_seq))
    }

    async fn resize(
        &self,
        session_id: uuid::Uuid,
        cols: u16,
        rows: u16,
    ) -> Option<TerminalSessionHandle> {
        let mut sessions = self.sessions.lock().await;
        let entry = sessions.get_mut(&session_id)?;
        entry.disconnected_since = None;
        entry.cols = cols;
        entry.rows = rows;
        Some(entry.handle.clone())
    }

    async fn update_delivered_seq(&self, session_id: uuid::Uuid, next_seq: u64) {
        if let Some(entry) = self.sessions.lock().await.get_mut(&session_id) {
            entry.disconnected_since = None;
            entry.last_delivered_seq = entry.last_delivered_seq.max(next_seq);
        }
    }

    async fn mark_connected(&self) {
        for entry in self.sessions.lock().await.values_mut() {
            entry.disconnected_since = None;
        }
    }

    async fn mark_disconnected(&self) {
        let now = unix_now();
        for entry in self.sessions.lock().await.values_mut() {
            entry.disconnected_since.get_or_insert(now);
        }
    }

    async fn exact_session_disconnected_expired(
        &self,
        session_id: uuid::Uuid,
        open_job_id: uuid::Uuid,
    ) -> bool {
        let now = unix_now();
        self.sessions
            .lock()
            .await
            .get(&session_id)
            .filter(|entry| entry.handle.open_job_id == open_job_id)
            .and_then(|entry| {
                let disconnected_since = entry.disconnected_since?;
                let grace =
                    u64::from(entry.idle_timeout_secs.max(1)).min(TERMINAL_DISCONNECTED_GRACE_SECS);
                Some(now.saturating_sub(disconnected_since) >= grace)
            })
            .unwrap_or(false)
    }

    async fn remove_all(&self) -> Vec<TerminalRegistryEntry> {
        self.sessions
            .lock()
            .await
            .drain()
            .map(|(_, entry)| entry)
            .collect()
    }
}

struct TerminalRegistryEntry {
    handle: TerminalSessionHandle,
    last_delivered_seq: u64,
    last_input_seq: u64,
    disconnected_since: Option<u64>,
    idle_timeout_secs: u32,
    cols: u16,
    rows: u16,
    _capacity_owner: Option<OwnedSemaphorePermit>,
    _session_owner: Option<TerminalSessionOwner>,
}

struct TerminalSessionOwner(Option<tokio::task::AbortHandle>);

impl TerminalSessionOwner {
    fn disarm(&mut self) {
        self.0.take();
    }
}

struct TerminalSessionRunOwner(tokio::task::JoinHandle<()>);

impl Drop for TerminalSessionRunOwner {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Drop for TerminalSessionOwner {
    fn drop(&mut self) {
        if let Some(owner) = self.0.take() {
            owner.abort();
        }
    }
}

struct TerminalOutputBuffer {
    chunks: VecDeque<TerminalOutputChunk>,
    next_seq: u64,
    retained_bytes: usize,
    max_retained_bytes: usize,
    dropped_bytes: u64,
    dropped_chunks: u64,
}

impl TerminalOutputBuffer {
    fn new(max_retained_bytes: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            next_seq: 1,
            retained_bytes: 0,
            max_retained_bytes: max_retained_bytes.max(4096),
            dropped_bytes: 0,
            dropped_chunks: 0,
        }
    }

    fn push(&mut self, mut data: Vec<u8>) -> Option<(TerminalOutputChunk, TerminalOutputRange)> {
        if data.is_empty() {
            return None;
        }
        if data.len() > self.max_retained_bytes {
            self.dropped_bytes = self
                .dropped_bytes
                .saturating_add((data.len() - self.max_retained_bytes) as u64);
            data = data[data.len() - self.max_retained_bytes..].to_vec();
        }
        self.retained_bytes += data.len();
        let chunk = TerminalOutputChunk {
            seq: self.next_seq,
            data,
        };
        self.chunks.push_back(chunk.clone());
        self.next_seq = self.next_seq.saturating_add(1);
        while self.retained_bytes > self.max_retained_bytes {
            let Some(removed) = self.chunks.pop_front() else {
                self.retained_bytes = 0;
                break;
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(removed.data.len());
            self.dropped_bytes = self.dropped_bytes.saturating_add(removed.data.len() as u64);
            self.dropped_chunks = self.dropped_chunks.saturating_add(1);
        }
        let range = self.range_from(chunk.seq);
        Some((chunk, range))
    }

    fn snapshot_from(&self, from_seq: u64) -> TerminalOutputSnapshot {
        let retained_first_seq = self.chunks.front().map(|chunk| chunk.seq);
        let chunks = self
            .chunks
            .iter()
            .filter(|chunk| chunk.seq >= from_seq)
            .cloned()
            .collect::<Vec<_>>();
        let first_seq = chunks.first().map(|chunk| chunk.seq);
        TerminalOutputSnapshot {
            chunks,
            range: TerminalOutputRange {
                first_seq,
                next_seq: self.next_seq,
                retained_first_seq,
                retained_bytes: self.retained_bytes,
                dropped_bytes: self.dropped_bytes,
                dropped_chunks: self.dropped_chunks,
                replay_truncated: self.replay_truncated(from_seq, retained_first_seq),
            },
        }
    }

    fn range_from(&self, from_seq: u64) -> TerminalOutputRange {
        let retained_first_seq = self.chunks.front().map(|chunk| chunk.seq);
        let first_seq = self
            .chunks
            .iter()
            .find(|chunk| chunk.seq >= from_seq)
            .map(|chunk| chunk.seq);
        TerminalOutputRange {
            first_seq,
            next_seq: self.next_seq,
            retained_first_seq,
            retained_bytes: self.retained_bytes,
            dropped_bytes: self.dropped_bytes,
            dropped_chunks: self.dropped_chunks,
            replay_truncated: self.replay_truncated(from_seq, retained_first_seq),
        }
    }

    fn replay_truncated(&self, from_seq: u64, retained_first_seq: Option<u64>) -> bool {
        if self.dropped_bytes == 0 {
            return false;
        }
        retained_first_seq
            .map(|first_seq| from_seq < first_seq)
            .unwrap_or(from_seq < self.next_seq)
    }
}

#[derive(Clone)]
struct TerminalOutputChunk {
    seq: u64,
    data: Vec<u8>,
}

struct TerminalOutputSnapshot {
    chunks: Vec<TerminalOutputChunk>,
    range: TerminalOutputRange,
}

struct TerminalOutputRange {
    first_seq: Option<u64>,
    next_seq: u64,
    retained_first_seq: Option<u64>,
    retained_bytes: usize,
    dropped_bytes: u64,
    dropped_chunks: u64,
    replay_truncated: bool,
}

async fn collect_session_output(
    job_id: uuid::Uuid,
    session_id: uuid::Uuid,
    from_seq: Option<u64>,
) -> (Vec<CommandOutput>, TerminalOutputRange) {
    let Some((handle, start_seq)) = registry().collect_handle(session_id, from_seq).await else {
        return (
            Vec::new(),
            TerminalOutputRange {
                first_seq: None,
                next_seq: 0,
                retained_first_seq: None,
                retained_bytes: 0,
                dropped_bytes: 0,
                dropped_chunks: 0,
                replay_truncated: false,
            },
        );
    };
    let (outputs, range) = collect_output_from_handle(job_id, &handle, Some(start_seq)).await;
    registry()
        .update_delivered_seq(session_id, range.next_seq)
        .await;
    (outputs, range)
}

async fn collect_output_from_handle(
    job_id: uuid::Uuid,
    handle: &TerminalSessionHandle,
    from_seq: Option<u64>,
) -> (Vec<CommandOutput>, TerminalOutputRange) {
    let output = handle.output.lock().await;
    let snapshot = output.snapshot_from(from_seq.unwrap_or(output.next_seq));
    let outputs = snapshot
        .chunks
        .into_iter()
        .map(|chunk| CommandOutput {
            job_id,
            stream: OutputStream::Pty,
            data: chunk.data,
            exit_code: None,
            done: false,
        })
        .collect();
    (outputs, snapshot.range)
}

struct TerminalReaderOwner(tokio::task::JoinHandle<Result<()>>);

impl Drop for TerminalReaderOwner {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn read_terminal_output(
    mut reader: tokio::fs::File,
    handle: TerminalSessionHandle,
) -> Result<()> {
    let mut buffer = vec![0_u8; TERMINAL_READ_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                if !handle.has_stream_sender().await {
                    let _ = handle.output.lock().await.push(buffer[..read].to_vec());
                    continue;
                }
                let mut data = buffer[..read].to_vec();
                let mut eof = false;
                let settle = time::sleep(Duration::from_millis(TERMINAL_OUTPUT_SETTLE_MS));
                tokio::pin!(settle);
                while data.len() < TERMINAL_READ_CHUNK_BYTES * 2 {
                    tokio::select! {
                        next = reader.read(&mut buffer) => {
                            match next {
                                Ok(0) => {
                                    eof = true;
                                    break;
                                }
                                Ok(read) => data.extend_from_slice(&buffer[..read]),
                                Err(error) if error.raw_os_error() == Some(libc::EIO) => {
                                    eof = true;
                                    break;
                                }
                                Err(error) => return Err(error).context("terminal PTY read failed"),
                            }
                        }
                        _ = &mut settle => break,
                    }
                }
                let retained = handle.output.lock().await.push(data);
                if let Some((chunk, range)) = retained {
                    handle.emit_stream_chunk(chunk, range).await;
                    handle
                        .emit_stream_status("terminal_stream", "streaming", false, Some(0))
                        .await;
                }
                if eof {
                    break;
                }
            }
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => return Err(error).context("terminal PTY read failed"),
        }
    }
    Ok(())
}

async fn run_terminal_session(
    reader: tokio::fs::File,
    mut child: tokio::process::Child,
    handle: TerminalSessionHandle,
    idle_timeout_secs: u32,
) {
    let session_id = handle.session_id;
    let open_job_id = handle.open_job_id;
    let mut reader_owner =
        TerminalReaderOwner(tokio::spawn(read_terminal_output(reader, handle.clone())));
    let mut reader_finished = false;
    let mut child_finished = false;
    let idle_timeout_secs = u64::from(idle_timeout_secs.max(1));
    let mut idle_checks = time::interval(Duration::from_secs(
        idle_timeout_secs.min(TERMINAL_IDLE_SCAN_SECS),
    ));
    idle_checks.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    idle_checks.tick().await;
    loop {
        tokio::select! {
            reader_result = &mut reader_owner.0, if !reader_finished => {
                reader_finished = true;
                match reader_result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        warn!(%error, %session_id, %open_job_id, "terminal output consumer failed");
                        let _ = terminate_terminal_process_group(handle.process_group_id).await;
                    }
                    Err(error) => {
                        warn!(%error, %session_id, %open_job_id, "terminal output consumer panicked");
                        let _ = terminate_terminal_process_group(handle.process_group_id).await;
                    }
                }
            }
            wait_result = child.wait(), if !child_finished => {
                let (status, exit_code, reason) = match wait_result {
                    Ok(status) => ("exited", status.code(), None),
                    Err(error) => {
                        warn!(%error, %session_id, %open_job_id, "terminal child waiter failed");
                        ("failed", None, Some("terminal_child_wait_failed"))
                    }
                };
                *handle.exit_code.lock().await = Some(exit_code);
                handle
                    .emit_stream_status_with_reason(
                        "terminal_stream",
                        status,
                        true,
                        exit_code,
                        reason,
                    )
                    .await;
                child_finished = true;
            }
            _ = idle_checks.tick() => {
                if handle.session_exited().await {
                    handle
                        .emit_stream_status("terminal_stream", "exited", true, Some(0))
                        .await;
                    let _ = registry().remove_if_current(session_id, open_job_id).await;
                    return;
                }
                if registry()
                    .exact_session_disconnected_expired(session_id, open_job_id)
                    .await
                {
                    let _ = terminate_terminal_process_group(handle.process_group_id).await;
                    handle
                        .emit_stream_status_with_reason(
                            "terminal_stream",
                            "disconnected_timeout",
                            true,
                            Some(0),
                            Some("gateway_disconnected_timeout"),
                        )
                        .await;
                    let _ = registry().remove_if_current(session_id, open_job_id).await;
                    return;
                }
                let idle_for =
                    unix_now().saturating_sub(handle.last_activity.load(Ordering::Relaxed));
                if idle_for >= idle_timeout_secs {
                    let _ = terminate_terminal_process_group(handle.process_group_id).await;
                    handle
                        .emit_stream_status("terminal_stream", "idle_timeout", true, Some(124))
                        .await;
                    let _ = registry().remove_if_current(session_id, open_job_id).await;
                    return;
                }
            }
        }
    }
}

async fn close_removed_terminal_entry(
    entry: TerminalRegistryEntry,
    event_type: &'static str,
    status: &'static str,
    reason: &str,
) {
    if let Err(error) = terminate_terminal_process_group(entry.handle.process_group_id).await {
        warn!(
            %error,
            session_id = %entry.handle.session_id,
            "terminal lifecycle process cleanup failed"
        );
    }
    time::sleep(Duration::from_millis(TERMINAL_OUTPUT_SETTLE_MS)).await;
    entry
        .handle
        .emit_stream_status_with_reason(event_type, status, true, Some(0), Some(reason))
        .await;
}

fn with_status(
    mut outputs: Vec<CommandOutput>,
    job_id: uuid::Uuid,
    status: serde_json::Value,
    exit_code: Option<i32>,
) -> Vec<CommandOutput> {
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status).unwrap_or_default(),
        exit_code,
        done: false,
    });
    outputs
}

fn status_with_output_range(
    mut status: serde_json::Value,
    range: &TerminalOutputRange,
) -> serde_json::Value {
    if let Some(object) = status.as_object_mut() {
        object.insert(
            "output_first_seq".to_string(),
            serde_json::json!(range.first_seq),
        );
        object.insert(
            "output_next_seq".to_string(),
            serde_json::json!(range.next_seq),
        );
        object.insert(
            "output_retained_first_seq".to_string(),
            serde_json::json!(range.retained_first_seq),
        );
        object.insert(
            "output_retained_bytes".to_string(),
            serde_json::json!(range.retained_bytes),
        );
        object.insert(
            "output_dropped_bytes".to_string(),
            serde_json::json!(range.dropped_bytes),
        );
        object.insert(
            "output_dropped_chunks".to_string(),
            serde_json::json!(range.dropped_chunks),
        );
        object.insert(
            "output_replay_truncated".to_string(),
            serde_json::json!(range.replay_truncated),
        );
    }
    status
}

fn status_output(
    job_id: uuid::Uuid,
    status: serde_json::Value,
    exit_code: Option<i32>,
) -> CommandOutput {
    CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status).unwrap_or_default(),
        exit_code,
        done: true,
    }
}

fn validate_terminal_argv(argv: &[String]) -> Result<()> {
    if argv.is_empty() {
        anyhow::bail!("terminal argv is empty");
    }
    if !argv[0].starts_with('/') {
        anyhow::bail!("terminal executable must be absolute");
    }
    if argv.iter().any(|part| part.is_empty() || part.len() > 4096) {
        anyhow::bail!("terminal argv contains an invalid part");
    }
    Ok(())
}

fn validate_terminal_cwd(cwd: Option<&str>) -> Result<()> {
    let Some(cwd) = cwd else {
        return Ok(());
    };
    if cwd.len() > 4096 || !Path::new(cwd).is_absolute() {
        anyhow::bail!("terminal cwd must be absolute and bounded");
    }
    Ok(())
}

fn validate_terminal_reason(reason: Option<&str>) -> Result<()> {
    if let Some(reason) = reason {
        if reason.len() > MAX_TERMINAL_REASON_BYTES {
            anyhow::bail!("terminal close reason is too large");
        }
    }
    Ok(())
}

async fn terminate_terminal_process_group(
    process_group_id: libc::pid_t,
) -> Result<ProcessCleanupReport> {
    tokio::task::spawn_blocking(move || {
        terminate_process_group_blocking(
            process_group_id,
            Duration::from_millis(TERMINAL_CLOSE_GRACE_MS),
        )
    })
    .await
    .context("terminal process cleanup task failed")
}

fn registry() -> &'static TerminalRegistry {
    TERMINAL_REGISTRY.get_or_init(|| TerminalRegistry {
        sessions: Mutex::new(HashMap::new()),
    })
}

fn terminal_open_owners() -> &'static StdMutex<HashMap<uuid::Uuid, Arc<Mutex<()>>>> {
    TERMINAL_OPEN_OWNERS.get_or_init(|| StdMutex::new(HashMap::new()))
}

async fn acquire_terminal_open_owner(session_id: uuid::Uuid) -> TerminalOpenOwner {
    let entry = {
        let mut owners = terminal_open_owners()
            .lock()
            .expect("terminal-open owner registry poisoned");
        owners
            .entry(session_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let mut owner = TerminalOpenOwner {
        session_id,
        entry: entry.clone(),
        guard: None,
    };
    owner.guard = Some(entry.lock_owned().await);
    owner
}

fn terminal_session_capacity() -> Arc<Semaphore> {
    TERMINAL_SESSION_CAPACITY
        .get_or_init(|| Arc::new(Semaphore::new(MAX_TERMINAL_SESSIONS)))
        .clone()
}

fn pending_final_events() -> &'static Mutex<TerminalPendingFinalEvents> {
    TERMINAL_PENDING_FINAL_EVENTS.get_or_init(|| Mutex::new(TerminalPendingFinalEvents::default()))
}

fn pending_final_notify() -> &'static Notify {
    TERMINAL_PENDING_FINAL_NOTIFY.get_or_init(Notify::new)
}

#[cfg(test)]
pub(crate) async fn terminal_session_is_registered(session_id: uuid::Uuid) -> bool {
    registry().sessions.lock().await.contains_key(&session_id)
}

#[cfg(test)]
#[path = "tests_terminal.rs"]
mod tests;
