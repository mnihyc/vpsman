use std::{
    error::Error,
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use anyhow::Result;
use serde_json::json;
use vpsman_common::{CommandOutput, OutputStream};

#[derive(Clone, Debug, Default)]
pub(crate) struct CommandCancelToken {
    canceled: Arc<AtomicBool>,
    reason: Arc<Mutex<Option<String>>>,
}

impl CommandCancelToken {
    pub(crate) fn cancel(&self, reason: String) {
        if let Ok(mut current) = self.reason.lock() {
            *current = Some(reason);
        }
        self.canceled.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }

    pub(crate) fn reason(&self) -> String {
        self.reason
            .lock()
            .ok()
            .and_then(|current| current.clone())
            .unwrap_or_else(|| "canceled".to_string())
    }

    pub(crate) fn check(&self, operation_type: &'static str) -> Result<()> {
        if self.is_canceled() {
            return Err(CommandCanceled {
                operation_type,
                reason: self.reason(),
            }
            .into());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct CommandCanceled {
    operation_type: &'static str,
    reason: String,
}

impl CommandCanceled {
    pub(crate) fn operation_type(&self) -> &'static str {
        self.operation_type
    }

    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for CommandCanceled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "command canceled: operation_type={} reason={}",
            self.operation_type, self.reason
        )
    }
}

impl Error for CommandCanceled {}

pub(crate) fn command_canceled_output(
    job_id: uuid::Uuid,
    operation_type: &str,
    reason: &str,
) -> Result<CommandOutput> {
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&json!({
            "type": "command_canceled",
            "operation_type": operation_type,
            "reason": reason,
        }))?,
        exit_code: Some(130),
        done: true,
    })
}

pub(crate) fn command_timeout_output(
    job_id: uuid::Uuid,
    operation_type: &str,
    timeout_secs: u64,
) -> Result<CommandOutput> {
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&json!({
            "type": "command_timeout",
            "operation_type": operation_type,
            "timeout_secs": timeout_secs.max(1),
        }))?,
        exit_code: Some(124),
        done: true,
    })
}
