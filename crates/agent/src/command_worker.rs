use std::{
    error::Error,
    fmt,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use anyhow::Result;
use serde_json::json;
use tokio::sync::Notify;
use vpsman_common::{CommandOutput, OutputStream};

#[derive(Clone)]
pub(crate) struct CommandCancelToken {
    canceled: Arc<AtomicBool>,
    reason: Arc<Mutex<Option<String>>>,
    notify: Arc<Notify>,
}

impl Default for CommandCancelToken {
    fn default() -> Self {
        Self {
            canceled: Arc::new(AtomicBool::new(false)),
            reason: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
        }
    }
}

impl fmt::Debug for CommandCancelToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandCancelToken")
            .field("canceled", &self.is_canceled())
            .field("reason", &self.reason())
            .finish()
    }
}

impl CommandCancelToken {
    pub(crate) fn cancel(&self, reason: String) {
        if let Ok(mut current) = self.reason.lock() {
            *current = Some(reason);
        }
        self.canceled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
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
            return Err(CommandCanceled::new(operation_type, self.reason()).into());
        }
        Ok(())
    }

    pub(crate) async fn cancelled(&self) -> String {
        loop {
            let notified = self.notify.notified();
            if self.is_canceled() {
                return self.reason();
            }
            notified.await;
        }
    }
}

#[derive(Debug)]
pub(crate) struct CommandCanceled {
    operation_type: &'static str,
    reason: String,
}

impl CommandCanceled {
    pub(crate) fn new(operation_type: &'static str, reason: String) -> Self {
        Self {
            operation_type,
            reason,
        }
    }

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

pub(crate) async fn run_cancelable<T, F>(
    operation_type: &'static str,
    cancel_token: CommandCancelToken,
    operation: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    cancel_token.check(operation_type)?;
    tokio::select! {
        biased;
        reason = cancel_token.cancelled() => Err(CommandCanceled::new(operation_type, reason).into()),
        result = operation => result,
    }
}

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
    max_timeout_secs: u64,
) -> Result<CommandOutput> {
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&json!({
            "type": "command_timeout",
            "operation_type": operation_type,
            "max_timeout_secs": max_timeout_secs.max(1),
        }))?,
        exit_code: Some(124),
        done: true,
    })
}
