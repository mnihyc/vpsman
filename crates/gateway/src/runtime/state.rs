use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
    time::Instant,
};

use tokio::sync::{mpsc, oneshot, watch, Mutex, RwLock};
use vpsman_common::{
    CommandOutput, GatewayCommandCancelResult, GatewayCommandDispatchResult, JobAck, JobCancelAck,
    JobCancelRequest, JobRequest, PrivilegeAssertionReplayCache, TerminalControlAck,
    TerminalControlRequest,
};

use crate::api_client::GatewayForwardMetrics;

const MAX_RETAINED_COMMAND_OUTPUTS: usize = 256;
const MAX_RETAINED_COMMAND_OUTPUT_BYTES: usize = 1024 * 1024;
pub(crate) const SESSION_COMMAND_QUEUE_CAPACITY: usize = 1024;

#[derive(Clone)]
pub(crate) struct GatewayState {
    pub(crate) sessions: Arc<RwLock<HashMap<String, GatewaySession>>>,
    pub(crate) client_lifecycle_owners: Arc<GatewayClientLifecycleOwners>,
    pub(crate) client_suspension_fences: Arc<RwLock<HashMap<String, GatewayClientSuspensionFence>>>,
    /// Recent dispatch markers are indexed by their lifecycle owner so a
    /// suspension fence reads only that client's jobs. Global expiry cleanup
    /// remains a background maintenance concern.
    pub(crate) command_enqueues:
        Arc<RwLock<HashMap<String, HashMap<uuid::Uuid, GatewayCommandEnqueueMarker>>>>,
    pub(crate) privilege_assertions: Arc<Mutex<PrivilegeAssertionReplayCache>>,
    pub(crate) disconnected_at: Arc<RwLock<HashMap<String, Instant>>>,
    pub(crate) forward_metrics: Arc<GatewayForwardMetrics>,
    pub(crate) reconnect_grace_secs: Arc<AtomicU64>,
    pub(crate) dispatch_ack_secs: Arc<AtomicU64>,
}

impl Default for GatewayState {
    fn default() -> Self {
        Self {
            sessions: Arc::default(),
            client_lifecycle_owners: Arc::default(),
            client_suspension_fences: Arc::default(),
            command_enqueues: Arc::default(),
            privilege_assertions: Arc::default(),
            disconnected_at: Arc::default(),
            forward_metrics: Arc::default(),
            reconnect_grace_secs: Arc::new(AtomicU64::new(60)),
            dispatch_ack_secs: Arc::new(AtomicU64::new(30)),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GatewayClientSuspensionFence {
    pub(crate) token: uuid::Uuid,
    /// `None` is the persistent, post-commit state. A prepared fence expires
    /// automatically if its API caller dies before the database mutation.
    pub(crate) expires_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GatewayCommandEnqueueMarker {
    pub(crate) generation: uuid::Uuid,
    pub(crate) expires_at: Instant,
}

impl GatewayClientSuspensionFence {
    pub(crate) fn active_at(self, now: Instant) -> bool {
        self.expires_at.is_none_or(|expires_at| expires_at > now)
    }
}

impl GatewayState {
    pub(crate) async fn client_lifecycle_owner(&self, client_id: &str) -> Arc<RwLock<()>> {
        self.client_lifecycle_owners.owner(client_id).await
    }

    pub(crate) fn reconnect_grace_secs(&self) -> u64 {
        self.reconnect_grace_secs.load(Ordering::Relaxed)
    }

    pub(crate) fn dispatch_ack_secs(&self) -> u64 {
        self.dispatch_ack_secs.load(Ordering::Relaxed)
    }

    pub(crate) fn set_runtime_timing(&self, reconnect_grace_secs: u64, dispatch_ack_secs: u64) {
        self.reconnect_grace_secs
            .store(reconnect_grace_secs.max(1), Ordering::Relaxed);
        self.dispatch_ack_secs
            .store(dispatch_ack_secs.max(1), Ordering::Relaxed);
    }

    pub(crate) async fn prune_expired_command_enqueues(&self, now: Instant) -> usize {
        let mut enqueues = self.command_enqueues.write().await;
        let before = enqueues.values().map(HashMap::len).sum::<usize>();
        enqueues.retain(|_, client_enqueues| {
            client_enqueues.retain(|_, marker| marker.expires_at > now);
            !client_enqueues.is_empty()
        });
        let after = enqueues.values().map(HashMap::len).sum::<usize>();
        before.saturating_sub(after)
    }
}

#[derive(Default)]
pub(crate) struct GatewayClientLifecycleOwners {
    owners: Mutex<HashMap<String, Weak<RwLock<()>>>>,
}

impl GatewayClientLifecycleOwners {
    async fn owner(&self, client_id: &str) -> Arc<RwLock<()>> {
        let mut owners = self.owners.lock().await;
        owners.retain(|_, owner| owner.strong_count() > 0);
        if let Some(owner) = owners.get(client_id).and_then(Weak::upgrade) {
            return owner;
        }
        let owner = Arc::new(RwLock::new(()));
        owners.insert(client_id.to_string(), Arc::downgrade(&owner));
        owner
    }
}

#[derive(Clone)]
pub(crate) struct GatewaySession {
    pub(crate) session_id: uuid::Uuid,
    pub(crate) process_incarnation_id: uuid::Uuid,
    pub(crate) sender: mpsc::Sender<GatewaySessionMessage>,
    pub(crate) close_tx: watch::Sender<Option<GatewaySessionCloseRequest>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GatewaySessionCloseRequest {
    Graceful(String),
    Immediate(String),
}

pub(crate) enum GatewaySessionMessage {
    Command(Box<GatewayCommand>),
    Cancel(GatewayCancelCommand),
    TerminalControl(GatewayTerminalControlCommand),
}

pub(crate) struct GatewayCommand {
    pub(crate) request: JobRequest,
    pub(crate) payload_hash: String,
    pub(crate) response: oneshot::Sender<GatewayCommandDispatchResult>,
}

pub(crate) struct GatewayCancelCommand {
    pub(crate) request: JobCancelRequest,
    pub(crate) response: oneshot::Sender<GatewayCommandCancelResult>,
}

pub(crate) struct GatewayTerminalControlCommand {
    pub(crate) request: TerminalControlRequest,
    pub(crate) response: oneshot::Sender<TerminalControlAck>,
}

pub(crate) struct PendingCommand {
    pub(crate) client_id: String,
    pub(crate) job_id: uuid::Uuid,
    pub(crate) command_version: u16,
    pub(crate) payload_hash: String,
    pub(crate) ack: Option<JobAck>,
    pub(crate) outputs: Vec<CommandOutput>,
    pub(crate) response: Option<oneshot::Sender<GatewayCommandDispatchResult>>,
}

impl PendingCommand {
    pub(crate) fn retain_output_if_response_waiting(&mut self, output: CommandOutput) -> u64 {
        if self.response.is_none() {
            return 0;
        }
        self.outputs.push(output);
        let mut dropped = 0_u64;
        while self.outputs.len() > MAX_RETAINED_COMMAND_OUTPUTS
            || retained_output_bytes(&self.outputs) > MAX_RETAINED_COMMAND_OUTPUT_BYTES
        {
            if self.outputs.is_empty() {
                break;
            }
            self.outputs.remove(0);
            dropped = dropped.saturating_add(1);
        }
        dropped
    }
}

pub(crate) fn finish_pending_command_response(
    pending: &mut PendingCommand,
    ack_override: Option<JobAck>,
    outputs_override: Vec<CommandOutput>,
) {
    let ack = ack_override.or(pending.ack.take()).unwrap_or(JobAck {
        job_id: pending.job_id,
        accepted: false,
        message: "command completed without ack".to_string(),
    });
    let outputs = if outputs_override.is_empty() {
        std::mem::take(&mut pending.outputs)
    } else {
        outputs_override
    };
    let Some(response) = pending.response.take() else {
        return;
    };
    let _ = response.send(GatewayCommandDispatchResult {
        client_id: pending.client_id.clone(),
        job_id: pending.job_id,
        command_version: pending.command_version,
        accepted: ack.accepted,
        message: ack.message,
        outputs,
    });
}

pub(crate) fn cancel_ack_result(
    client_id: String,
    ack: JobCancelAck,
) -> GatewayCommandCancelResult {
    GatewayCommandCancelResult {
        client_id,
        job_id: ack.job_id,
        acked: true,
        accepted: ack.accepted,
        applied: ack.applied,
        message: ack.message,
    }
}

fn retained_output_bytes(outputs: &[CommandOutput]) -> usize {
    outputs
        .iter()
        .map(|output| output.data.len().saturating_add(64))
        .sum()
}

#[cfg(test)]
#[path = "tests_state.rs"]
mod tests;
