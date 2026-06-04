use uuid::Uuid;

pub(crate) const CANCELED_STATUS: &str = "canceled";
pub(crate) const CANCEL_REQUESTED_STATUS: &str = "cancel_requested";
pub(crate) const CANCELABLE_PENDING_STATUSES: &[&str] = &["approval_required"];
pub(crate) const CANCELABLE_ACTIVE_STATUSES: &[&str] = &["dispatching", CANCEL_REQUESTED_STATUS];

#[derive(Clone, Debug)]
pub(crate) struct JobCancellationRecord {
    pub(crate) job_id: Uuid,
    pub(crate) canceled: bool,
    pub(crate) status: String,
    pub(crate) canceled_targets: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveJobCancellationRecord {
    pub(crate) job_id: Uuid,
    pub(crate) requested: bool,
    pub(crate) status: String,
    pub(crate) target_clients: Vec<String>,
}

pub(crate) fn job_status_is_cancelable(status: &str) -> bool {
    CANCELABLE_PENDING_STATUSES.contains(&status)
}

pub(crate) fn job_status_is_active_cancelable(status: &str) -> bool {
    CANCELABLE_ACTIVE_STATUSES.contains(&status)
}
