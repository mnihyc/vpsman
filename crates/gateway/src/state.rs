use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use vpsman_common::{
    CommandOutput, GatewayCommandCancelResult, GatewayCommandDispatchResult, JobAck, JobCancelAck,
    JobCancelRequest, JobRequest, PrivilegeAssertionReplayCache,
};

use crate::api_client::GatewayForwardMetrics;

const MAX_RETAINED_COMMAND_OUTPUTS: usize = 256;
const MAX_RETAINED_COMMAND_OUTPUT_BYTES: usize = 1024 * 1024;
pub(crate) const SESSION_COMMAND_QUEUE_CAPACITY: usize = 1024;

#[derive(Clone)]
pub(crate) struct GatewayState {
    pub(crate) sessions: Arc<RwLock<HashMap<String, GatewaySession>>>,
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
            privilege_assertions: Arc::default(),
            disconnected_at: Arc::default(),
            forward_metrics: Arc::default(),
            reconnect_grace_secs: Arc::new(AtomicU64::new(60)),
            dispatch_ack_secs: Arc::new(AtomicU64::new(30)),
        }
    }
}

impl GatewayState {
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
}

#[derive(Clone)]
pub(crate) struct GatewaySession {
    pub(crate) session_id: uuid::Uuid,
    pub(crate) process_incarnation_id: uuid::Uuid,
    pub(crate) sender: mpsc::Sender<GatewaySessionMessage>,
}

pub(crate) enum GatewaySessionMessage {
    Command(GatewayCommand),
    Cancel(GatewayCancelCommand),
    Disconnect(String),
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
mod tests {
    use super::*;
    use vpsman_common::OutputStream;

    #[test]
    fn pending_command_does_not_retain_output_after_ack_response_is_consumed() {
        let job_id = uuid::Uuid::new_v4();
        let (response, _receiver) = oneshot::channel();
        let mut pending = PendingCommand {
            client_id: "client-a".to_string(),
            job_id,
            command_version: 1,
            payload_hash: "payload-a".to_string(),
            ack: Some(JobAck {
                job_id,
                accepted: true,
                message: "accepted".to_string(),
            }),
            outputs: Vec::new(),
            response: Some(response),
        };

        finish_pending_command_response(&mut pending, None, Vec::new());
        let dropped = pending.retain_output_if_response_waiting(CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: b"noisy output after ack".to_vec(),
            exit_code: None,
            done: false,
        });

        assert_eq!(dropped, 0);
        assert!(pending.response.is_none());
        assert!(pending.outputs.is_empty());
    }

    #[test]
    fn pending_command_reports_retained_output_truncation() {
        let job_id = uuid::Uuid::new_v4();
        let (response, _receiver) = oneshot::channel();
        let mut pending = PendingCommand {
            client_id: "client-a".to_string(),
            job_id,
            command_version: 1,
            payload_hash: "payload-a".to_string(),
            ack: None,
            outputs: Vec::new(),
            response: Some(response),
        };

        let mut dropped = 0_u64;
        for _ in 0..(MAX_RETAINED_COMMAND_OUTPUTS + 2) {
            dropped += pending.retain_output_if_response_waiting(CommandOutput {
                job_id,
                stream: OutputStream::Stdout,
                data: b"line\n".to_vec(),
                exit_code: None,
                done: false,
            });
        }

        assert_eq!(dropped, 2);
        assert_eq!(pending.outputs.len(), MAX_RETAINED_COMMAND_OUTPUTS);
    }
}
