use serde::Serialize;

use crate::{
    auth_model::OperatorView,
    model::{
        AgentView, AuditLogView, FleetAlertView, FleetSummary, JobHistoryView, ScheduleView,
        TelemetryNetworkRateView, TelemetryRollupView,
    },
    model_backups::{BackupArtifactView, BackupRequestView},
    model_dashboard::{DashboardOverviewView, SystemDashboardView},
    model_file_transfer::FileTransferSessionView,
    model_fleet_snapshot::FleetSnapshotSource,
    model_monitoring::MonitoringCardView,
    model_terminal::TerminalSessionView,
};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HomeSnapshotResponse {
    pub(crate) generated_at: String,
    pub(crate) operator: OperatorView,
    pub(crate) summary: FleetSnapshotSource<FleetSummary>,
    pub(crate) agents: FleetSnapshotSource<Vec<AgentView>>,
    pub(crate) telemetry_rollups: FleetSnapshotSource<Vec<TelemetryRollupView>>,
    pub(crate) telemetry_network_rates: FleetSnapshotSource<Vec<TelemetryNetworkRateView>>,
    pub(crate) fleet_alerts: FleetSnapshotSource<Vec<FleetAlertView>>,
    pub(crate) fleet_alerts_truncated: bool,
    pub(crate) monitoring_cards: FleetSnapshotSource<Vec<MonitoringCardView>>,
    pub(crate) jobs: FleetSnapshotSource<Vec<JobHistoryView>>,
    pub(crate) file_transfers: FleetSnapshotSource<Vec<FileTransferSessionView>>,
    pub(crate) terminal_sessions: FleetSnapshotSource<Vec<TerminalSessionView>>,
    pub(crate) backups: FleetSnapshotSource<Vec<BackupRequestView>>,
    pub(crate) backup_artifacts: FleetSnapshotSource<Vec<BackupArtifactView>>,
    pub(crate) audit: FleetSnapshotSource<Vec<AuditLogView>>,
    pub(crate) schedules: FleetSnapshotSource<Vec<ScheduleView>>,
    pub(crate) system_dashboard: FleetSnapshotSource<SystemDashboardView>,
    pub(crate) dashboard_overview: FleetSnapshotSource<DashboardOverviewView>,
}
