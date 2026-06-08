use std::{collections::HashMap, sync::Arc};

use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use vpsman_common::{
    CommandOutput, GatewayCommandCancelResult, GatewayCommandDispatchResult, JobAck, JobRequest,
    PrivilegeAssertionReplayCache,
};

#[derive(Clone, Default)]
pub(crate) struct GatewayState {
    pub(crate) sessions: Arc<RwLock<HashMap<String, GatewaySession>>>,
    pub(crate) privilege_assertions: Arc<Mutex<PrivilegeAssertionReplayCache>>,
}

#[derive(Clone)]
pub(crate) struct GatewaySession {
    pub(crate) session_id: uuid::Uuid,
    pub(crate) sender: mpsc::Sender<GatewayCommand>,
    pub(crate) cancel_sender: mpsc::Sender<GatewayCommandCancel>,
}

pub(crate) struct GatewayCommand {
    pub(crate) request: JobRequest,
    pub(crate) response: oneshot::Sender<GatewayCommandDispatchResult>,
}

pub(crate) struct GatewayCommandCancel {
    pub(crate) client_id: String,
    pub(crate) job_id: uuid::Uuid,
    pub(crate) reason: Option<String>,
    pub(crate) response: oneshot::Sender<GatewayCommandCancelResult>,
}

pub(crate) struct PendingCommand {
    pub(crate) client_id: String,
    pub(crate) job_id: uuid::Uuid,
    pub(crate) command_version: u16,
    pub(crate) ack: Option<JobAck>,
    pub(crate) outputs: Vec<CommandOutput>,
    pub(crate) next_output_seq: i32,
    pub(crate) response: oneshot::Sender<GatewayCommandDispatchResult>,
}

pub(crate) fn finish_pending_command(
    pending_command: &mut Option<PendingCommand>,
    ack_override: Option<JobAck>,
    outputs_override: Vec<CommandOutput>,
) {
    let Some(mut pending) = pending_command.take() else {
        return;
    };
    let ack = ack_override.or(pending.ack.take()).unwrap_or(JobAck {
        job_id: pending.job_id,
        accepted: false,
        message: "command completed without ack".to_string(),
    });
    let outputs = if outputs_override.is_empty() {
        pending.outputs
    } else {
        outputs_override
    };
    let _ = pending.response.send(GatewayCommandDispatchResult {
        client_id: pending.client_id,
        job_id: pending.job_id,
        command_version: pending.command_version,
        accepted: ack.accepted,
        message: ack.message,
        outputs,
    });
}
