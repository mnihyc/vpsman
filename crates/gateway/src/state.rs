use std::{collections::HashMap, sync::Arc, time::Instant};

use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use vpsman_common::{
    CommandOutput, GatewayCommandCancelResult, GatewayCommandDispatchResult, JobAck, JobCancelAck,
    JobCancelRequest, JobRequest, PrivilegeAssertionReplayCache,
};

use crate::api_client::GatewayForwardMetrics;

#[derive(Clone)]
pub(crate) struct GatewayState {
    pub(crate) sessions: Arc<RwLock<HashMap<String, GatewaySession>>>,
    pub(crate) privilege_assertions: Arc<Mutex<PrivilegeAssertionReplayCache>>,
    pub(crate) disconnected_at: Arc<RwLock<HashMap<String, Instant>>>,
    pub(crate) forward_metrics: Arc<GatewayForwardMetrics>,
    pub(crate) reconnect_grace_secs: u64,
}

impl Default for GatewayState {
    fn default() -> Self {
        Self {
            sessions: Arc::default(),
            privilege_assertions: Arc::default(),
            disconnected_at: Arc::default(),
            forward_metrics: Arc::default(),
            reconnect_grace_secs: 60,
        }
    }
}

#[derive(Clone)]
pub(crate) struct GatewaySession {
    pub(crate) session_id: uuid::Uuid,
    pub(crate) sender: mpsc::UnboundedSender<GatewaySessionMessage>,
}

pub(crate) enum GatewaySessionMessage {
    Command(GatewayCommand),
    Cancel(GatewayCancelCommand),
}

pub(crate) struct GatewayCommand {
    pub(crate) request: JobRequest,
    pub(crate) response: oneshot::Sender<GatewayCommandDispatchResult>,
}

pub(crate) struct GatewayCancelCommand {
    pub(crate) request: JobCancelRequest,
    pub(crate) response: oneshot::Sender<GatewayCommandCancelResult>,
}

pub(crate) struct PendingCommand {
    pub(crate) client_id: String,
    pub(crate) job_id: uuid::Uuid,
    pub(crate) command_version: u16,
    pub(crate) ack: Option<JobAck>,
    pub(crate) outputs: Vec<CommandOutput>,
    pub(crate) next_output_seq: i32,
    pub(crate) response: Option<oneshot::Sender<GatewayCommandDispatchResult>>,
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
