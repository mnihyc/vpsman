use serde::Serialize;
use uuid::Uuid;
use vpsman_common::HostProcessView;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HostJobAttemptView {
    pub(crate) job_id: Uuid,
    pub(crate) status: String,
    pub(crate) message: Option<String>,
    pub(crate) completed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HostProcessInventoryView {
    pub(crate) client_id: String,
    pub(crate) source_job_id: Option<Uuid>,
    pub(crate) source: Option<String>,
    pub(crate) truncated: bool,
    pub(crate) observed_at: Option<String>,
    pub(crate) processes: Vec<HostProcessView>,
    pub(crate) last_attempt: Option<HostJobAttemptView>,
}

#[derive(Clone, Debug)]
pub(crate) struct HostJobEvidence {
    pub(crate) latest_attempt: Option<HostJobAttemptView>,
    pub(crate) latest_success_job_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HostServiceInventoryView {
    pub(crate) client_id: String,
    pub(crate) source_job_id: Option<Uuid>,
    pub(crate) observed_at: Option<String>,
    pub(crate) capability: Option<vpsman_common::HostServiceCapability>,
    pub(crate) truncated: bool,
    pub(crate) services: Vec<vpsman_common::HostServiceRecord>,
    pub(crate) last_attempt: Option<HostJobAttemptView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HostStorageInventoryView {
    pub(crate) client_id: String,
    pub(crate) source_job_id: Option<Uuid>,
    pub(crate) observed_at: Option<String>,
    pub(crate) capability: Option<vpsman_common::HostStorageCapability>,
    pub(crate) include_pseudo_mounts: bool,
    pub(crate) devices_truncated: bool,
    pub(crate) mounts_truncated: bool,
    pub(crate) devices: Vec<vpsman_common::HostBlockDeviceRecord>,
    pub(crate) mounts: Vec<vpsman_common::HostMountRecord>,
    pub(crate) last_attempt: Option<HostJobAttemptView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HostPackageUpdatePlanView {
    pub(crate) client_id: String,
    pub(crate) source_job_id: Option<Uuid>,
    pub(crate) observed_at: Option<String>,
    pub(crate) capability: Option<vpsman_common::HostPackageCapability>,
    pub(crate) metadata_refresh_requested: bool,
    pub(crate) metadata_refreshed: bool,
    pub(crate) plan_hash: Option<String>,
    pub(crate) truncated: bool,
    pub(crate) packages: Vec<vpsman_common::HostPackageUpdateRecord>,
    pub(crate) reboot_required_before: Option<bool>,
    pub(crate) last_attempt: Option<HostJobAttemptView>,
    pub(crate) evidence_error: Option<String>,
}
