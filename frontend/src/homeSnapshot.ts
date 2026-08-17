import type {
  AgentView,
  AuditLogRecord,
  BackupArtifactRecord,
  BackupRequestRecord,
  DashboardOverviewRecord,
  FleetAlertRecord,
  FleetSummary,
  JobHistoryRecord,
  MonitoringCardView,
  OperatorView,
  ScheduleRecord,
  SystemDashboardRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
} from "./types";
import type { FileTransferSessionRecord } from "./typesFileTransfer";
import type { TerminalSessionRecord } from "./typesTerminal";

export type SnapshotSource<T> = {
  data: T | null;
  error: string | null;
};

export type HomeSnapshotRecord = {
  generated_at: string;
  operator: OperatorView;
  summary: SnapshotSource<FleetSummary>;
  agents: SnapshotSource<AgentView[]>;
  telemetry_rollups: SnapshotSource<TelemetryRollupRecord[]>;
  telemetry_network_rates: SnapshotSource<TelemetryNetworkRateRecord[]>;
  fleet_alerts: SnapshotSource<FleetAlertRecord[]>;
  monitoring_cards: SnapshotSource<MonitoringCardView[]>;
  jobs: SnapshotSource<JobHistoryRecord[]>;
  file_transfers: SnapshotSource<FileTransferSessionRecord[]>;
  terminal_sessions: SnapshotSource<TerminalSessionRecord[]>;
  backups: SnapshotSource<BackupRequestRecord[]>;
  backup_artifacts: SnapshotSource<BackupArtifactRecord[]>;
  audit: SnapshotSource<AuditLogRecord[]>;
  schedules: SnapshotSource<ScheduleRecord[]>;
  system_dashboard: SnapshotSource<SystemDashboardRecord>;
  dashboard_overview: SnapshotSource<DashboardOverviewRecord>;
};

export function unavailableSnapshotSource<T>(error: string): SnapshotSource<T> {
  return { data: null, error };
}

export function snapshotSourceAvailable<T>(
  source: SnapshotSource<T>,
): source is { data: T; error: null } {
  return source.data !== null && source.error === null;
}

export function snapshotSourceError(
  label: string,
  source: SnapshotSource<unknown>,
): string | null {
  return snapshotSourceAvailable(source)
    ? null
    : `${label}: ${source.error ?? "snapshot source unavailable"}`;
}
