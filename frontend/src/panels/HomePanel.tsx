import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  ArrowRight,
  Clock3,
  DatabaseBackup,
  FolderOpen,
  Network,
  Play,
  ShieldAlert,
  TerminalSquare,
  UserPlus,
} from "lucide-react";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { ActionFeedback } from "../components/ActionFeedback";
import { VpsCombobox } from "../components/VpsCombobox";
import { agentDisplayState } from "../agentDisplayState";
import {
  formatLowerBoundCount,
  isActionableFleetAlertState,
} from "../constants";
import type { FileTransferSessionRecord } from "../typesFileTransfer";
import type {
  AgentView,
  AuditLogRecord,
  BackupArtifactRecord,
  BackupRequestRecord,
  DashboardDrilldownRecord,
  DashboardPreferences,
  DashboardWindow,
  FleetAlertRecord,
  FleetSummary,
  JobHistoryRecord,
  SystemDashboardRecord,
  ScheduleRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
} from "../types";
import { displayNameOrUnnamed, formatCompactTime, formatFullTime, shortId } from "../utils";

type HomePanelProps = {
  agents: AgentView[];
  allAgents: AgentView[];
  auditLogs: AuditLogRecord[];
  backupArtifacts: BackupArtifactRecord[];
  backups: BackupRequestRecord[];
  backupsEvidenceAvailable: boolean;
  dashboardError: string | null;
  dashboardLoading: boolean;
  dashboardPreferences: DashboardPreferences;
  dashboardWindow: DashboardWindow;
  fileTransfers: FileTransferSessionRecord[];
  fleetAlertsEvidenceAvailable: boolean;
  fleetAlerts: FleetAlertRecord[];
  fleetCoreEvidenceAvailable: boolean;
  homeEvidenceComplete: boolean;
  jobs: JobHistoryRecord[];
  jobsEvidenceAvailable: boolean;
  recordBounds: {
    backupArtifacts: boolean;
    backups: boolean;
    fileTransfers: boolean;
    fleetAlerts: boolean;
    jobs: boolean;
  };
  schedules: ScheduleRecord[];
  summary: FleetSummary;
  systemDashboard: SystemDashboardRecord | null;
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  onDashboardNavigate: (drilldown: DashboardDrilldownRecord) => void;
  onDashboardPreferencesChange: (patch: Partial<DashboardPreferences>) => void;
  onDashboardRefresh: () => void;
  onDashboardWindowChange: (window: DashboardWindow) => void;
  onOpenAudit: () => void;
  onOpenBackup: (agent: AgentView) => void;
  onOpenBackups: () => void;
  onOpenDispatch: (agent: AgentView) => void;
  onOpenFiles: (agent: AgentView) => void;
  onOpenFleetAlerts: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenJobs: () => void;
  onOpenNetwork: (agent: AgentView) => void;
  onOpenNetworkEvidence: (agent?: AgentView) => void;
  onOpenProcesses: (agent: AgentView) => void;
  onOpenSchedule: () => void;
  onOpenSystemCapacity: () => void;
  onOpenTerminal: (agent: AgentView) => void;
  onOpenTransfers: () => void;
  onOpenVpsDetail: (agent: AgentView) => void;
  onRegisterVps: () => void;
  scopeFiltered: boolean;
};

type HomeActionItem = {
  detail: string;
  id: string;
  label: string;
  meta: string;
  metaTitle?: string;
  onOpen: () => void;
  tone: "critical" | "warning" | "info" | "ok";
};

type HomeActivityItem = {
  id: string;
  label: string;
  meta: string;
  onOpen: () => void;
  time: string;
  timeDateTime?: string;
  timeTitle?: string;
  type: string;
};

export function HomePanel({
  agents,
  allAgents,
  auditLogs,
  backupArtifacts,
  backups,
  backupsEvidenceAvailable,
  dashboardError,
  dashboardLoading,
  fileTransfers,
  fleetAlertsEvidenceAvailable,
  fleetAlerts,
  fleetCoreEvidenceAvailable,
  homeEvidenceComplete,
  jobs,
  jobsEvidenceAvailable,
  recordBounds,
  schedules,
  scopeFiltered,
  summary,
  systemDashboard,
  onOpenAudit,
  onOpenBackup,
  onOpenBackups,
  onOpenDispatch,
  onOpenFiles,
  onOpenFleetAlerts,
  onOpenJobDetails,
  onOpenJobs,
  onOpenNetwork,
  onOpenNetworkEvidence,
  onOpenSchedule,
  onOpenSystemCapacity,
  onOpenTerminal,
  onOpenTransfers,
  onOpenVpsDetail,
  onRegisterVps,
}: HomePanelProps) {
  const [quickTargetId, setQuickTargetId] = useState("");
  const quickTarget = agents.find((agent) => agent.id === quickTargetId) ?? agents[0] ?? null;
  const visibleDisplayStates = useMemo(
    () => agents.map((agent) => agentDisplayState(agent)),
    [agents],
  );
  const visibleOnline = visibleDisplayStates.filter((state) => state.label === "Online").length;
  const visibleContactUnknown = visibleDisplayStates.filter((state) => state.label === "Contact unknown").length;
  const visibleStale = visibleDisplayStates.filter((state) => state.label === "Stale").length;
  const visibleOffline = visibleDisplayStates.filter((state) => state.label === "Offline").length;
  const visibleReview = visibleDisplayStates.filter(
    (state) => state.tone === "warning" || state.tone === "critical",
  ).length;
  const loadedRunningJobs = jobs.filter((job) => isActiveJobStatus(job.status)).length;
  const runningJobs = Math.max(loadedRunningJobs, summary.running_jobs);
  const alertsTruncated = recordBounds.fleetAlerts;
  const runningJobsTruncated = recordBounds.jobs;
  const backupsTruncated = recordBounds.backups;
  const failedJobs = jobs.filter((job) => isFailedJobStatus(job.status)).length;
  const failedBackups = backups.filter((backup) => isFailedBackupStatus(backup.status)).length;
  const activeTransfers = fileTransfers.filter((transfer) => isActiveTransferStatus(transfer.status)).length;
  const activeAlerts = fleetAlerts.filter(
    (alert) => isActionableFleetAlertState(alert.operator_state),
  );
  const criticalAlerts = activeAlerts.filter((alert) => alert.severity === "critical").length;
  const warningAlerts = activeAlerts.filter((alert) => alert.severity === "warning").length;
  const infoAlerts = activeAlerts.length - criticalAlerts - warningAlerts;

  useEffect(() => {
    if (agents.length === 0) {
      setQuickTargetId("");
      return;
    }
    if (!agents.some((agent) => agent.id === quickTargetId)) {
      setQuickTargetId(agents[0].id);
    }
  }, [agents, quickTargetId]);

  const attentionItems = useMemo(
    () =>
      buildAttentionItems({
        agents,
        backups,
        fileTransfers,
        fleetAlerts,
        jobs,
        onOpenBackup,
        onOpenFleetAlerts,
        onOpenJobDetails,
        onOpenNetworkEvidence,
        onOpenTransfers,
        onOpenSystemCapacity,
        onOpenVpsDetail,
        scopeFiltered,
        systemDashboard,
      }),
    [
      agents,
      backups,
      fileTransfers,
      fleetAlerts,
      jobs,
      onOpenBackup,
      onOpenFleetAlerts,
      onOpenJobDetails,
      onOpenNetworkEvidence,
      onOpenTransfers,
      onOpenSystemCapacity,
      onOpenVpsDetail,
      scopeFiltered,
      systemDashboard,
    ],
  );
  const activityItems = useMemo(
    () =>
      buildActivityItems({
        auditLogs,
        backups,
        fileTransfers,
        jobs,
        onOpenAudit,
        onOpenBackups,
        onOpenJobDetails,
        onOpenSchedule,
        onOpenTransfers,
        schedules,
        scopeFiltered,
      }),
    [
      auditLogs,
      backups,
      fileTransfers,
      jobs,
      onOpenAudit,
      onOpenBackups,
      onOpenJobDetails,
      onOpenSchedule,
      onOpenTransfers,
      schedules,
      scopeFiltered,
    ],
  );
  const runningWorkItems = useMemo(
    () =>
      buildRunningWorkItems({
        backups,
        fileTransfers,
        jobs,
        onOpenJobs,
        onOpenBackups,
        onOpenJobDetails,
        onOpenTransfers,
        runningJobCount: runningJobs,
        scopeFiltered,
      }),
    [
      backups,
      fileTransfers,
      jobs,
      onOpenBackups,
      onOpenJobDetails,
      onOpenJobs,
      onOpenTransfers,
      runningJobs,
      scopeFiltered,
    ],
  );
  const recentFailureItems = useMemo(
    () =>
      buildRecentFailureItems({
        backups,
        fileTransfers,
        fleetAlerts,
        jobs,
        onOpenBackups,
        onOpenFleetAlerts,
        onOpenJobDetails,
        onOpenTransfers,
        scopeFiltered,
      }),
    [
      backups,
      fileTransfers,
      fleetAlerts,
      jobs,
      onOpenBackups,
      onOpenFleetAlerts,
      onOpenJobDetails,
      onOpenTransfers,
      scopeFiltered,
    ],
  );

  return (
    <div className="homeWorkspace">
      <ActionFeedback
        className="localActionFeedback"
        message={
          dashboardError ??
          (dashboardLoading ? "Refreshing dashboard evidence" : null)
        }
        tone={dashboardError ? "danger" : "progress"}
      />
      <section className="homeReleaseLayer" aria-labelledby="home-release-title">
        <div className="homeCommandBand">
          <div className="homeCommandIntro">
            <h2 id="home-release-title">Fleet command home</h2>
            <p>
              Scan VPS health, pick a target, and jump into reviewed operations without hunting through subsystem pages.
            </p>
            <div className="homeInlineStatus" aria-label="Home fleet posture">
              <ConsoleStatusBadge
                tone={
                  fleetCoreEvidenceAvailable
                    ? visibleOnline === agents.length && criticalAlerts === 0
                      ? "ok"
                      : "warning"
                    : "neutral"
                }
              >
                {fleetCoreEvidenceAvailable
                  ? `${visibleOnline}/${agents.length} visible live`
                  : "Fleet status unavailable"}
              </ConsoleStatusBadge>
              <ConsoleStatusBadge
                tone={
                  !fleetAlertsEvidenceAvailable
                    ? "neutral"
                    : criticalAlerts > 0
                      ? "critical"
                      : warningAlerts > 0
                        ? "warning"
                        : infoAlerts > 0 || alertsTruncated
                          ? "info"
                          : "ok"
                }
              >
                {fleetAlertsEvidenceAvailable
                  ? `${criticalAlerts} critical / ${warningAlerts} warning / ${infoAlerts} info${alertsTruncated ? " in loaded page" : ""}`
                  : "Alert evidence unavailable"}
              </ConsoleStatusBadge>
              <ConsoleStatusBadge
                tone={
                  !jobsEvidenceAvailable
                    ? "neutral"
                    : runningJobs > 0 || runningJobsTruncated
                      ? "info"
                      : "neutral"
                }
              >
                {jobsEvidenceAvailable
                  ? `${formatLowerBoundCount(runningJobs, runningJobsTruncated)} ${
                      scopeFiltered ? "fleet " : ""
                    }running jobs`
                  : "Job evidence unavailable"}
              </ConsoleStatusBadge>
            </div>
          </div>
          <div className="homeQuickActions" aria-label="Home quick actions">
            <label>
              <span>Target VPS</span>
              <VpsCombobox
                agents={agents}
                ariaLabel="Home quick action target"
                onChange={setQuickTargetId}
                placeholder="Select VPS"
                value={quickTarget?.id ?? ""}
              />
            </label>
            <div className="homeQuickActionGrid">
              <button
                className="primaryAction compactAction"
                aria-label="Open terminal for selected VPS"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenTerminal(quickTarget)}
                title="Open terminal for the selected VPS"
                type="button"
              >
                <TerminalSquare size={16} />
                <span>Terminal</span>
              </button>
              <button
                className="secondaryAction compactAction"
                aria-label="Browse files on selected VPS"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenFiles(quickTarget)}
                title="Browse files on the selected VPS"
                type="button"
              >
                <FolderOpen size={16} />
                <span>Files</span>
              </button>
              <button
                className="secondaryAction compactAction"
                aria-label="Dispatch command to selected VPS"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenDispatch(quickTarget)}
                title="Dispatch a command to the selected VPS"
                type="button"
              >
                <Play size={16} />
                <span>Command</span>
              </button>
              <button
                className="secondaryAction compactAction"
                aria-label="Run backup on selected VPS"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenBackup(quickTarget)}
                title="Run a backup on the selected VPS"
                type="button"
              >
                <DatabaseBackup size={16} />
                <span>Backup</span>
              </button>
              <button
                className="secondaryAction compactAction"
                aria-label="View network for selected VPS"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenNetwork(quickTarget)}
                title="View network state for the selected VPS"
                type="button"
              >
                <Network size={16} />
                <span>Network</span>
              </button>
            </div>
            {!quickTarget && (
              <div className="homeQuietState" aria-label="Home empty scope notice">
                <ShieldAlert size={18} />
                <span>
                  {!fleetCoreEvidenceAvailable
                    ? "Fleet inventory is unavailable. Retry before assuming no VPS is registered or changing identities."
                    : allAgents.length === 0
                    ? "No VPS is registered yet. Register the first identity, then run its generated install command."
                    : "No VPS matches the current scope. Adjust the fleet scope to restore quick actions."}
                </span>
                {fleetCoreEvidenceAvailable && allAgents.length === 0 ? (
                  <button
                    className="primaryAction compactAction"
                    onClick={onRegisterVps}
                    title="Open VPS identity registration"
                    type="button"
                  >
                    <UserPlus size={15} />
                    Register VPS
                  </button>
                ) : null}
              </div>
            )}
          </div>
        </div>

        <div className="homePostureStrip" aria-label="Home posture strip">
          <HomePostureMetric
            detail={
              fleetCoreEvidenceAvailable
                ? `${visibleOnline} live, ${visibleContactUnknown} contact unknown`
                : "Fleet status evidence is unavailable"
            }
            label="Live VPS"
            tone={
              !fleetCoreEvidenceAvailable
                ? "neutral"
                : visibleOnline === agents.length
                  ? "ok"
                  : "warning"
            }
            value={
              fleetCoreEvidenceAvailable
                ? `${visibleOnline}/${agents.length}`
                : "Unknown"
            }
          />
          <HomePostureMetric
            detail={
              fleetCoreEvidenceAvailable
                ? `${visibleStale} stale, ${visibleContactUnknown} contact unknown, ${visibleOffline} offline`
                : "Reachability evidence is unavailable"
            }
            label="Reachability gaps"
            tone={
              !fleetCoreEvidenceAvailable
                ? "neutral"
                : visibleReview || visibleOffline
                  ? "warning"
                  : "ok"
            }
            value={
              fleetCoreEvidenceAvailable
                ? String(visibleReview + visibleOffline)
                : "Unknown"
            }
          />
          <HomePostureMetric
            detail={
              fleetAlertsEvidenceAvailable
                ? `${criticalAlerts} critical, ${warningAlerts} warning, ${infoAlerts} info${alertsTruncated ? " in loaded page" : ""}`
                : "Fleet alert evidence is unavailable"
            }
            label="Open alerts"
            tone={
              !fleetAlertsEvidenceAvailable
                ? "neutral"
                : criticalAlerts
                  ? "critical"
                  : warningAlerts
                    ? "warning"
                    : infoAlerts || alertsTruncated
                      ? "info"
                      : "ok"
            }
            value={
              fleetAlertsEvidenceAvailable
                ? formatLowerBoundCount(activeAlerts.length, alertsTruncated)
                : "Unknown"
            }
          />
          <HomePostureMetric
            detail={
              jobsEvidenceAvailable
                ? `${formatLowerBoundCount(failedJobs, runningJobsTruncated)} failed in ${scopeFiltered ? "fleet " : ""}loaded history`
                : "Job history evidence is unavailable"
            }
            label={scopeFiltered ? "Fleet jobs" : "Running jobs"}
            tone={
              !jobsEvidenceAvailable
                ? "neutral"
                : failedJobs
                  ? "critical"
                  : runningJobs || runningJobsTruncated
                    ? "info"
                    : "ok"
            }
            value={
              jobsEvidenceAvailable
                ? formatLowerBoundCount(runningJobs, runningJobsTruncated)
                : "Unknown"
            }
          />
          <HomePostureMetric
            detail={
              backupsEvidenceAvailable
                ? `${formatLowerBoundCount(failedBackups, backupsTruncated)} failed${backupsTruncated ? " in loaded history" : ""}, ${formatLowerBoundCount(backupArtifacts.length, recordBounds.backupArtifacts)} artifacts${recordBounds.backupArtifacts ? " in the loaded page" : ""}`
                : "Backup request or artifact evidence is unavailable"
            }
            label="Backups"
            tone={
              !backupsEvidenceAvailable
                ? "neutral"
                : failedBackups
                  ? "critical"
                  : backupsTruncated || recordBounds.backupArtifacts
                    ? "info"
                    : "ok"
            }
            value={
              backupsEvidenceAvailable
                ? formatLowerBoundCount(backups.length, backupsTruncated)
                : "Unknown"
            }
          />
          <HomePostureMetric
            detail={
              jobsEvidenceAvailable
                ? `${formatLowerBoundCount(activeTransfers, recordBounds.fileTransfers)} active transfer sessions${recordBounds.fileTransfers ? " in loaded history" : ""}`
                : "File-transfer evidence is unavailable"
            }
            label="Transfers"
            tone={
              !jobsEvidenceAvailable
                ? "neutral"
                : activeTransfers || recordBounds.fileTransfers
                  ? "info"
                  : "ok"
            }
            value={
              jobsEvidenceAvailable
                ? formatLowerBoundCount(
                    fileTransfers.length,
                    recordBounds.fileTransfers,
                  )
                : "Unknown"
            }
          />
        </div>

        <div className="homeWorkGrid">
          <HomeActionPanel
            badge={
              homeEvidenceComplete
                ? `${runningWorkItems.length} shown`
                : `${runningWorkItems.length} shown · evidence incomplete`
            }
            emptyIcon={<Clock3 size={18} />}
            emptyText={
              homeEvidenceComplete
                ? "No running jobs, transfers, or backup requests in loaded records."
                : "No running work found in available evidence; some home sources are unavailable."
            }
            evidenceComplete={homeEvidenceComplete}
            id="home-running-work-title"
            items={runningWorkItems}
            subtitle={
              scopeFiltered
                ? "Scoped backup and transfer work, plus fleet-wide jobs that may need follow-up."
                : "Long-running jobs and transfer work that may need follow-up."
            }
            title={scopeFiltered ? "Running work and fleet jobs" : "Running work"}
          />
          <HomeActionPanel
            badge={
              homeEvidenceComplete
                ? `${recentFailureItems.length} shown`
                : `${recentFailureItems.length} shown · evidence incomplete`
            }
            emptyIcon={<ShieldAlert size={18} />}
            emptyText={
              homeEvidenceComplete
                ? "No recent failures or unacknowledged warning alerts in loaded records."
                : "No recent issues found in available evidence; some home sources are unavailable."
            }
            evidenceComplete={homeEvidenceComplete}
            id="home-recent-issues-title"
            items={recentFailureItems}
            subtitle={
              scopeFiltered
                ? "Scoped failures and alerts, plus fleet-wide failed jobs routed to their owner pages."
                : "Failed work and active warning alerts routed to their owner pages."
            }
            title="Recent issues"
          />
        </div>

        <div className="homeReviewGrid">
          <section className="homeReviewPanel" aria-labelledby="home-attention-title">
            <div className="homePanelHeader">
              <div>
                <h2 id="home-attention-title">Needs attention</h2>
                <span>
                  {scopeFiltered
                    ? "Scoped VPS evidence plus fleet-wide job and control-plane risks."
                    : "Failed work, stale agents, backup risk, degraded network, and access capability gaps."}
                </span>
              </div>
              <ConsoleStatusBadge
                tone={
                  attentionItems.length
                    ? "warning"
                    : homeEvidenceComplete
                      ? "ok"
                      : "neutral"
                }
              >
                {homeEvidenceComplete
                  ? `${attentionItems.length} shown`
                  : `${attentionItems.length} shown · evidence incomplete`}
              </ConsoleStatusBadge>
            </div>
            {attentionItems.length === 0 ? (
              <div
                className={`homeQuietState${homeEvidenceComplete ? "" : " evidenceIncomplete"}`}
              >
                <ShieldAlert size={18} />
                <span>
                  {homeEvidenceComplete
                    ? "No loaded evidence needs attention."
                    : "No issue found in available evidence; some home sources are unavailable."}
                </span>
              </div>
            ) : (
              <div className="homeActionList">
                {attentionItems.map((item) => (
                  <button className={`homeActionRow ${item.tone}`} key={item.id} onClick={item.onOpen} type="button">
                    <span className="homeActionGlyph" aria-hidden="true">
                      {item.tone === "critical" ? <AlertTriangle size={16} /> : <Activity size={16} />}
                    </span>
                    <span className="homeActionText">
                      <strong>{item.label}</strong>
                      <small>{item.detail}</small>
                    </span>
                    <span className="homeActionMeta" title={item.metaTitle}>
                      {item.meta}
                    </span>
                    <ArrowRight size={15} />
                  </button>
                ))}
              </div>
            )}
          </section>

          <section className="homeReviewPanel" aria-labelledby="home-activity-title">
            <div className="homePanelHeader">
              <div>
                <h2 id="home-activity-title">Recent activity</h2>
                <span>
                  {scopeFiltered
                    ? "Scoped backup, transfer, and schedule evidence plus fleet-wide audit and job activity."
                    : "Audit, job, backup, transfer, and schedule evidence from loaded records."}
                </span>
              </div>
              <ConsoleStatusBadge tone="neutral">
                {homeEvidenceComplete
                  ? `${activityItems.length} shown`
                  : `${activityItems.length} shown · evidence incomplete`}
              </ConsoleStatusBadge>
            </div>
            {activityItems.length === 0 ? (
              <div
                className={`homeQuietState${homeEvidenceComplete ? "" : " evidenceIncomplete"}`}
              >
                <Clock3 size={18} />
                <span>
                  {homeEvidenceComplete
                    ? "No recent activity loaded."
                    : "No recent activity found in available evidence; some home sources are unavailable."}
                </span>
              </div>
            ) : (
              <div className="homeActivityList">
                {activityItems.map((item) => (
                  <button className="homeActivityRow" key={item.id} onClick={item.onOpen} type="button">
                    <span className="homeActivityType">{item.type}</span>
                    <span className="homeActivityText">
                      <strong>{item.label}</strong>
                      <small>{item.meta}</small>
                    </span>
                    <time dateTime={item.timeDateTime} title={item.timeTitle}>
                      {item.time}
                    </time>
                  </button>
                ))}
              </div>
            )}
          </section>
        </div>
      </section>

    </div>
  );
}

function HomePostureMetric({
  detail,
  label,
  tone,
  value,
}: {
  detail: string;
  label: string;
  tone: "critical" | "warning" | "info" | "ok" | "neutral";
  value: string;
}) {
  return (
    <div className={`homePostureMetric ${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function HomeActionPanel({
  badge,
  emptyIcon,
  emptyText,
  evidenceComplete,
  id,
  items,
  subtitle,
  title,
}: {
  badge: string;
  emptyIcon: ReactNode;
  emptyText: string;
  evidenceComplete: boolean;
  id: string;
  items: HomeActionItem[];
  subtitle: string;
  title: string;
}) {
  return (
    <section className="homeReviewPanel" aria-labelledby={id}>
      <div className="homePanelHeader">
        <div>
          <h2 id={id}>{title}</h2>
          <span>{subtitle}</span>
        </div>
        <ConsoleStatusBadge
          tone={!evidenceComplete ? "neutral" : items.length ? "info" : "ok"}
        >
          {badge}
        </ConsoleStatusBadge>
      </div>
      {items.length === 0 ? (
        <div
          className={`homeQuietState${evidenceComplete ? "" : " evidenceIncomplete"}`}
        >
          {emptyIcon}
          <span>{emptyText}</span>
        </div>
      ) : (
        <div className="homeActionList">
          {items.map((item) => (
            <button className={`homeActionRow ${item.tone}`} key={item.id} onClick={item.onOpen} type="button">
              <span className="homeActionGlyph" aria-hidden="true">
                {item.tone === "critical" ? <AlertTriangle size={16} /> : <Activity size={16} />}
              </span>
              <span className="homeActionText">
                <strong>{item.label}</strong>
                <small>{item.detail}</small>
              </span>
              <span className="homeActionMeta" title={item.metaTitle}>
                {item.meta}
              </span>
              <ArrowRight size={15} />
            </button>
          ))}
        </div>
      )}
    </section>
  );
}

function buildAttentionItems({
  agents,
  backups,
  fileTransfers,
  fleetAlerts,
  jobs,
  onOpenBackup,
  onOpenFleetAlerts,
  onOpenJobDetails,
  onOpenNetworkEvidence,
  onOpenTransfers,
  onOpenSystemCapacity,
  onOpenVpsDetail,
  scopeFiltered,
  systemDashboard,
}: {
  agents: AgentView[];
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  onOpenBackup: (agent: AgentView) => void;
  onOpenFleetAlerts: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenNetworkEvidence: (agent?: AgentView) => void;
  onOpenTransfers: () => void;
  onOpenSystemCapacity: () => void;
  onOpenVpsDetail: (agent: AgentView) => void;
  scopeFiltered: boolean;
  systemDashboard: SystemDashboardRecord | null;
}): HomeActionItem[] {
  const agentById = new Map(agents.map((agent) => [agent.id, agent]));
  const alertItems = fleetAlerts
    .filter(
      (alert) =>
        isActionableFleetAlertState(alert.operator_state) &&
        (alert.severity === "critical" || alert.severity === "warning"),
    )
    .map((alert) => {
      const alertAgent = alert.client_id ? agentById.get(alert.client_id) : undefined;
      return {
        detail: `${alert.category} / ${alert.client_id ? displayNameOrUnnamed(alertAgent?.display_name ?? alert.client_id) : alert.target_id}`,
        id: `alert:${alert.id}`,
        label: alert.title,
        meta: formatCompactTime(alert.observed_at),
        metaTitle: formatFullTime(alert.observed_at),
        onOpen:
          alert.category === "network"
            ? () => onOpenNetworkEvidence(alertAgent)
            : onOpenFleetAlerts,
        tone: alert.severity === "critical" ? "critical" : "warning",
      } satisfies HomeActionItem;
    });
  const agentItems = agents
    .map((agent) => ({ agent, displayState: agentDisplayState(agent) }))
    .filter(
      ({ agent, displayState }) =>
        displayState.label !== "Online" ||
        agent.stale_since ||
        agent.capabilities.privilege_mode === "unknown",
    )
    .map(({ agent, displayState }) => ({
      detail:
        agent.stale_reason ??
        `${displayState.detail}; privilege ${agent.capabilities.privilege_mode}`,
      id: `agent:${agent.id}`,
      label: `${displayNameOrUnnamed(agent.display_name)} needs review`,
      meta: agent.last_seen_at ? formatCompactTime(agent.last_seen_at) : "no heartbeat",
      metaTitle: agent.last_seen_at ? formatFullTime(agent.last_seen_at) : undefined,
      onOpen: () => onOpenVpsDetail(agent),
      tone: displayState.label === "Offline" ? "critical" : "warning",
    }) satisfies HomeActionItem);
  const jobItems = jobs
    .filter((job) => isFailedJobStatus(job.status))
    .map((job) => ({
      detail: `${job.command_type} / ${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
      id: `job:${job.id}`,
      label: `${scopeFiltered ? "Fleet job" : "Job"} ${shortId(job.id)} failed`,
      meta: formatCompactTime(job.completed_at ?? job.created_at),
      metaTitle: formatFullTime(job.completed_at ?? job.created_at),
      onOpen: () => onOpenJobDetails(job.id),
      tone: "critical",
    }) satisfies HomeActionItem);
  const transferItems = fileTransfers
    .filter((transfer) => transfer.status === "aborted" || transfer.status === "unknown")
    .map((transfer) => ({
      detail: `${transfer.direction} ${transfer.path}`,
      id: `transfer:${transfer.client_id}:${transfer.session_id}`,
      label: `Transfer ${shortId(transfer.session_id)} needs retry`,
      meta: formatCompactTime(transfer.observed_at),
      metaTitle: formatFullTime(transfer.observed_at),
      onOpen: onOpenTransfers,
      tone: transfer.status === "unknown" ? "warning" : "critical",
    }) satisfies HomeActionItem);
  const backupItems = backups
    .filter((backup) => isFailedBackupStatus(backup.status))
    .map((backup) => {
      const agent = agentById.get(backup.client_id);
      return {
        detail: `${displayNameOrUnnamed(agent?.display_name ?? backup.client_id)} / ${backup.paths.join(", ")}`,
        id: `backup:${backup.id}`,
        label: `Backup ${shortId(backup.id)} failed`,
        meta: formatCompactTime(backup.created_at),
        metaTitle: formatFullTime(backup.created_at),
        onOpen: () => (agent ? onOpenBackup(agent) : undefined),
        tone: "critical",
      } satisfies HomeActionItem;
    });
  const systemItems = buildSystemAttentionItems(systemDashboard, onOpenSystemCapacity);
  return [...alertItems, ...agentItems, ...jobItems, ...transferItems, ...backupItems, ...systemItems]
    .sort(compareAttentionItems)
    .slice(0, 8);
}

function buildSystemAttentionItems(
  systemDashboard: SystemDashboardRecord | null,
  onOpenSystemCapacity: () => void,
): HomeActionItem[] {
  if (!systemDashboard) {
    return [];
  }
  const dispatch = systemDashboard.current.dispatch;
  const gateway = systemDashboard.current.gateway_events;
  const droppedEvents = (gateway.dropped_events ?? 0) + (gateway.telemetry_dropped_events ?? 0);
  const criticalGatewayFailures = gateway.critical_failures ?? 0;
  const dispatchQueueDepth = dispatch.queue_depth;
  const gatewayQueueDepth = gateway.current_queue_depth ?? 0;
  const dispatchWarningThreshold = systemDashboard.capacity.dispatcher_in_flight
    ? Math.ceil(systemDashboard.capacity.dispatcher_in_flight * 0.5)
    : null;
  const dispatchHardThreshold =
    systemDashboard.capacity.dispatcher_batch ??
    systemDashboard.capacity.dispatcher_in_flight;
  const gatewayOldestAgeSecs = gateway.oldest_event_age_secs;
  const dispatchQueueNeedsAttention =
    (dispatchHardThreshold !== null &&
      dispatchQueueDepth >= dispatchHardThreshold) ||
    (dispatchWarningThreshold !== null &&
      dispatchQueueDepth >= dispatchWarningThreshold);
  const gatewayQueueNeedsAttention =
    gatewayOldestAgeSecs !== null && gatewayOldestAgeSecs >= 60;
  const items: HomeActionItem[] = [];
  if (criticalGatewayFailures > 0 || droppedEvents > 0) {
    items.push({
      detail: `${droppedEvents} dropped events, ${criticalGatewayFailures} critical failures`,
      id: "system:gateway-drops",
      label: "Gateway event drops need review",
      meta: "System / Capacity",
      onOpen: onOpenSystemCapacity,
      tone: criticalGatewayFailures > 0 ? "critical" : "warning",
    });
  }
  if (dispatchQueueNeedsAttention || gatewayQueueNeedsAttention) {
    const critical =
      (dispatchHardThreshold !== null &&
        dispatchQueueDepth >= dispatchHardThreshold) ||
      (gatewayOldestAgeSecs !== null && gatewayOldestAgeSecs >= 300);
    items.push({
      detail: `${dispatch.active_jobs} active jobs, ${dispatchQueueDepth} dispatch queued, ${gatewayQueueDepth} gateway queued${gatewayOldestAgeSecs === null ? "" : `, oldest gateway event ${gatewayOldestAgeSecs}s`}`,
      id: "system:dispatch-queue",
      label: "Control-plane queue pressure",
      meta: "System / Capacity",
      onOpen: onOpenSystemCapacity,
      tone: critical ? "critical" : "warning",
    });
  }
  return items;
}

function buildRunningWorkItems({
  backups,
  fileTransfers,
  jobs,
  onOpenBackups,
  onOpenJobDetails,
  onOpenJobs,
  onOpenTransfers,
  runningJobCount,
  scopeFiltered,
}: {
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  jobs: JobHistoryRecord[];
  onOpenBackups: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenJobs: () => void;
  onOpenTransfers: () => void;
  runningJobCount: number;
  scopeFiltered: boolean;
}): HomeActionItem[] {
  const jobItems = jobs
    .filter((job) => isActiveJobStatus(job.status))
    .map((job) => ({
      detail: `${readableJobCommand(job.command_type)} / ${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
      id: `running-job:${job.id}`,
      label: `${scopeFiltered ? "Fleet job" : "Job"} ${shortId(job.id)} ${readableJobStatus(job.status)}`,
      meta: formatCompactTime(job.created_at),
      metaTitle: formatFullTime(job.created_at),
      onOpen: () => onOpenJobDetails(job.id),
      tone: "info",
    }) satisfies HomeActionItem);
  const transferItems = fileTransfers
    .filter((transfer) => isActiveTransferStatus(transfer.status))
    .map((transfer) => ({
      detail: `${readableTransferDirection(transfer.direction)} ${transfer.path}`,
      id: `running-transfer:${transfer.client_id}:${transfer.session_id}`,
      label: `Transfer ${shortId(transfer.session_id)} ${readableTransferStatus(transfer.status)}`,
      meta: formatCompactTime(transfer.observed_at),
      metaTitle: formatFullTime(transfer.observed_at),
      onOpen: onOpenTransfers,
      tone: "info",
    }) satisfies HomeActionItem);
  const backupItems = backups
    .filter((backup) => isActiveBackupStatus(backup.status))
    .map((backup) => ({
      detail: `${backup.client_id} / ${backup.paths.join(", ")}`,
      id: `running-backup:${backup.id}`,
      label: `Backup ${shortId(backup.id)} ${readableBackupStatus(backup.status)}`,
      meta: formatCompactTime(backup.created_at),
      metaTitle: formatFullTime(backup.created_at),
      onOpen: onOpenBackups,
      tone: "info",
    }) satisfies HomeActionItem);
  const summaryItems: HomeActionItem[] =
    runningJobCount > jobItems.length
      ? [
          {
            detail:
              jobItems.length > 0
                ? `${jobItems.length} active job record${jobItems.length === 1 ? "" : "s"} loaded; open Jobs for full target state.`
                : "Open Jobs for active target state and retained output.",
            id: "running-jobs:fleet-summary",
            label: `${runningJobCount} fleet job${runningJobCount === 1 ? "" : "s"} running`,
            meta: "Fleet summary",
            onOpen: onOpenJobs,
            tone: "info",
          },
        ]
      : [];
  return [...summaryItems, ...jobItems, ...transferItems, ...backupItems]
    .sort((left, right) => (right.metaTitle ?? "").localeCompare(left.metaTitle ?? ""))
    .slice(0, 6);
}

function buildRecentFailureItems({
  backups,
  fileTransfers,
  fleetAlerts,
  jobs,
  onOpenBackups,
  onOpenFleetAlerts,
  onOpenJobDetails,
  onOpenTransfers,
  scopeFiltered,
}: {
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  onOpenBackups: () => void;
  onOpenFleetAlerts: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenTransfers: () => void;
  scopeFiltered: boolean;
}): HomeActionItem[] {
  const jobItems = jobs
    .filter((job) => isFailedJobStatus(job.status))
    .map((job) => ({
      detail: `${readableJobCommand(job.command_type)} / ${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
      id: `failed-job:${job.id}`,
      label: `${scopeFiltered ? "Fleet job" : "Job"} ${shortId(job.id)} ${readableJobStatus(job.status)}`,
      meta: formatCompactTime(job.completed_at ?? job.created_at),
      metaTitle: formatFullTime(job.completed_at ?? job.created_at),
      onOpen: () => onOpenJobDetails(job.id),
      tone: "critical",
    }) satisfies HomeActionItem);
  const transferItems = fileTransfers
    .filter((transfer) => transfer.status === "aborted" || transfer.status === "unknown")
    .map((transfer) => ({
      detail: `${readableTransferDirection(transfer.direction)} ${transfer.path}`,
      id: `failed-transfer:${transfer.client_id}:${transfer.session_id}`,
      label: `Transfer ${shortId(transfer.session_id)} ${readableTransferStatus(transfer.status)}`,
      meta: formatCompactTime(transfer.observed_at),
      metaTitle: formatFullTime(transfer.observed_at),
      onOpen: onOpenTransfers,
      tone: transfer.status === "unknown" ? "warning" : "critical",
    }) satisfies HomeActionItem);
  const backupItems = backups
    .filter((backup) => isFailedBackupStatus(backup.status))
    .map((backup) => ({
      detail: `${backup.client_id} / ${backup.paths.join(", ")}`,
      id: `failed-backup:${backup.id}`,
      label: `Backup ${shortId(backup.id)} ${readableBackupStatus(backup.status)}`,
      meta: formatCompactTime(backup.created_at),
      metaTitle: formatFullTime(backup.created_at),
      onOpen: onOpenBackups,
      tone: "critical",
    }) satisfies HomeActionItem);
  const alertItems = fleetAlerts
    .filter(
      (alert) =>
        isActionableFleetAlertState(alert.operator_state) &&
        (alert.severity === "critical" || alert.severity === "warning"),
    )
    .map((alert) => ({
      detail: `${readableAlertCategory(alert.category)} / ${alert.client_id ?? alert.target_id}`,
      id: `failure-alert:${alert.id}`,
      label: alert.title,
      meta: formatCompactTime(alert.observed_at),
      metaTitle: formatFullTime(alert.observed_at),
      onOpen: onOpenFleetAlerts,
      tone: alert.severity === "critical" ? "critical" : "warning",
    }) satisfies HomeActionItem);
  return [...jobItems, ...transferItems, ...backupItems, ...alertItems]
    .sort(compareAttentionItems)
    .slice(0, 6);
}

function buildActivityItems({
  auditLogs,
  backups,
  fileTransfers,
  jobs,
  onOpenAudit,
  onOpenBackups,
  onOpenJobDetails,
  onOpenSchedule,
  onOpenTransfers,
  schedules,
  scopeFiltered,
}: {
  auditLogs: AuditLogRecord[];
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  jobs: JobHistoryRecord[];
  onOpenAudit: () => void;
  onOpenBackups: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenSchedule: () => void;
  onOpenTransfers: () => void;
  schedules: ScheduleRecord[];
  scopeFiltered: boolean;
}): HomeActivityItem[] {
  const jobItems = jobs.map((job) => ({
    id: `job:${job.id}`,
    label: `${readableJobCommand(job.command_type)} job ${readableJobStatus(job.status)}`,
    meta: `${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
    onOpen: () => onOpenJobDetails(job.id),
    time: job.completed_at ?? job.created_at,
    type: scopeFiltered ? "Fleet job" : "Job",
  }));
  const backupItems = backups.map((backup) => ({
    id: `backup:${backup.id}`,
    label: `Backup ${readableBackupStatus(backup.status)}`,
    meta: `${backup.client_id} / ${backup.paths.join(", ")}`,
    onOpen: onOpenBackups,
    time: backup.created_at,
    type: "Backup",
  }));
  const transferItems = fileTransfers.map((transfer) => ({
    id: `transfer:${transfer.client_id}:${transfer.session_id}`,
    label: `${readableTransferDirection(transfer.direction)} transfer ${readableTransferStatus(transfer.status)}`,
    meta: transfer.path,
    onOpen: onOpenTransfers,
    time: transfer.observed_at,
    type: "Transfer",
  }));
  const auditItems = auditLogs.map((audit) => ({
    id: `audit:${audit.id}`,
    label: readableAuditAction(audit.action),
    meta: audit.target,
    onOpen: onOpenAudit,
    time: audit.created_at,
    type: scopeFiltered ? "Fleet audit" : "Audit",
  }));
  const scheduleItems = schedules.map((schedule) => ({
    id: `schedule:${schedule.id}`,
    label: `${schedule.name} ${
      schedule.cadence_error
        ? "invalid cadence"
        : schedule.enabled
          ? "enabled"
          : "paused"
    }`,
    meta: `${readableJobCommand(schedule.command_type)} / ${schedule.selector_expression}`,
    onOpen: onOpenSchedule,
    time: schedule.updated_at,
    type: "Schedule",
  }));
  return [...jobItems, ...backupItems, ...transferItems, ...auditItems, ...scheduleItems]
    .sort((left, right) => new Date(right.time).getTime() - new Date(left.time).getTime())
    .slice(0, 8)
    .map((item) => ({
      ...item,
      timeDateTime: item.time,
      time: formatCompactTime(item.time),
      timeTitle: formatFullTime(item.time),
    }));
}

function compareAttentionItems(left: HomeActionItem, right: HomeActionItem) {
  return attentionRank(right.tone) - attentionRank(left.tone) || left.label.localeCompare(right.label);
}

function attentionRank(tone: HomeActionItem["tone"]) {
  if (tone === "critical") return 3;
  if (tone === "warning") return 2;
  if (tone === "info") return 1;
  return 0;
}

function isActiveJobStatus(status: string) {
  return ["queued", "dispatching", "running"].includes(status);
}

function isFailedJobStatus(status: string) {
  return ["failed", "rejected", "agent_lost", "agent_timeout", "control_timeout", "deadline_expired"].includes(status);
}

function isFailedBackupStatus(status: string) {
  return status === "execution_failed" || status === "execution_canceled";
}

function isActiveBackupStatus(status: string) {
  return ["queued", "running", "uploading", "collecting"].includes(status);
}

function isActiveTransferStatus(status: string) {
  return status === "started" || status === "transferring";
}

function readableJobCommand(commandType: string) {
  if (commandType === "shell_argv") {
    return "Argv command";
  }
  if (commandType === "scheduled_shell_argv") {
    return "Scheduled shell command";
  }
  if (commandType === "shell_pty") {
    return "Interactive shell";
  }
  return commandType
    .replace(/^scheduled_/, "scheduled ")
    .replace(/_/g, " ")
    .replace(/\bospf\b/gi, "OSPF")
    .replace(/\bvps\b/gi, "VPS")
    .replace(/\bapi\b/gi, "API")
    .replace(/\b[a-z]/g, (letter) => letter.toUpperCase());
}

function readableJobStatus(status: string) {
  const labels: Record<string, string> = {
    agent_lost: "agent lost",
    agent_timeout: "agent timeout",
    completed: "completed",
    control_timeout: "control timeout",
    deadline_expired: "deadline expired",
    dispatching: "dispatching",
    failed: "failed",
    queued: "queued",
    rejected: "rejected",
    running: "running",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function readableBackupStatus(status: string) {
  const labels: Record<string, string> = {
    artifact_metadata_recorded: "package linked",
    completed: "completed",
    execution_canceled: "canceled",
    execution_failed: "failed",
    queued: "queued",
    running: "running",
    uploading: "uploading",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function readableTransferDirection(direction: string) {
  if (direction === "upload") {
    return "Upload";
  }
  if (direction === "download") {
    return "Download";
  }
  return direction.replace(/_/g, " ");
}

function readableTransferStatus(status: string) {
  const labels: Record<string, string> = {
    aborted: "aborted",
    committed: "completed",
    started: "started",
    transferring: "transferring",
    unknown: "status unknown",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function readableAlertCategory(category: string) {
  return category
    .replace(/_/g, " ")
    .replace(/\bospf\b/gi, "OSPF")
    .replace(/\b[a-z]/g, (letter) => letter.toUpperCase());
}

function readableAuditAction(action: string) {
  return action
    .replace(/[._]/g, " ")
    .replace(/\bapi\b/gi, "API")
    .replace(/\b[a-z]/g, (letter) => letter.toUpperCase());
}
